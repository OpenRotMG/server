use super::*;
use crate::content::{PRIEST, WIZARD};
use crate::wire::Reader;
use std::time::Duration;

use super::tests::receive;
fn setup() -> (
    Directory,
    i32,
    i32,
    mpsc::Receiver<Packet>,
    mpsc::Receiver<Packet>,
) {
    let mut d = super::tests::directory();
    let (a, mut ar) = receive(&mut d, -2, 1, WIZARD);
    let (b, mut br) = receive(&mut d, -2, 2, PRIEST);
    let w = d.worlds.get_mut(&-2).unwrap();
    w.now = 600;
    w.updates(a, false).unwrap();
    w.updates(b, false).unwrap();
    while ar.try_recv().is_ok() {}
    while br.try_recv().is_ok() {}
    (d, a, b, ar, br)
}
#[test]
fn named_teleport_preserves_native_guards_and_shared_chat_throttle() {
    let (mut d, a, b, _ar, _br) = setup();
    let destination = d.worlds[&-2].players[&b].pos;
    let start = d.worlds[&-2].players[&a].pos;
    let now = Instant::now();
    for (mode, expected) in [
        "Cannot teleport while paused.",
        "Cannot teleport to a paused player.",
        "Teleport target is busy, dead, departing or quarantined.",
        "Teleport target is busy, dead, departing or quarantined.",
        "Teleport target is busy, dead, departing or quarantined.",
        "Teleport target has not been observed.",
        "Teleport target's position is blocked.",
        "Cannot teleport in this world.",
        "Position correction awaits GOTO acknowledgement.",
        "Busy, dead, departing or quarantined players cannot teleport.",
    ]
    .into_iter()
    .enumerate()
    {
        let w = d.worlds.get_mut(&-2).unwrap();
        w.name = if mode == 7 { "Vault" } else { "Nexus" }.into();
        let p = w.players.get_mut(&a).unwrap();
        p.paused = mode == 0;
        p.pending = mode == 9;
        if mode == 5 {
            p.known.remove(&b);
        } else {
            p.known.insert(b);
        }
        if mode == 8 {
            reconciliation::goto(p, true, vec![]);
        }
        let q = w.players.get_mut(&b).unwrap();
        q.character.name = "Benny".into();
        q.paused = mode == 1;
        q.pending = mode == 2;
        q.death_pending = mode == 3;
        q.persistence_failure = (mode == 4).then(|| "uncertain commit".into());
        q.pos = if mode == 6 {
            Pos { x: -1., y: -1. }
        } else {
            destination
        };
        assert_eq!(
            d.chat(
                a,
                "/teleport Benny".into(),
                now + Duration::from_secs(mode as u64)
            )
            .unwrap_err()
            .to_string(),
            expected
        );
        assert!(d.teleports.is_empty());
        assert_eq!(d.worlds[&-2].players[&a].pos.distance(start), 0.);
        if mode == 8 {
            d.worlds
                .get_mut(&-2)
                .unwrap()
                .acknowledge_goto(a, 0)
                .unwrap();
        }
    }
    d.worlds
        .get_mut(&-2)
        .unwrap()
        .players
        .get_mut(&a)
        .unwrap()
        .pending = false;
    let next = now + Duration::from_secs(10);
    for _ in 0..3 {
        assert_eq!(
            d.chat(a, "/teleport Missing".into(), next)
                .unwrap_err()
                .to_string(),
            "Player not found in this world."
        );
    }
    assert!(
        d.chat(a, "/who".into(), next)
            .unwrap_err()
            .to_string()
            .contains("Chat rate limit")
    );
    assert!(d.teleports.is_empty());
}

fn quest_setup() -> (
    Directory,
    i32,
    i32,
    mpsc::Receiver<Packet>,
    mpsc::Receiver<Packet>,
    i32,
    i32,
) {
    let (mut d, a, b, ar, br) = setup();
    let w = d.worlds.get_mut(&-2).unwrap();
    w.name = "Realm".into();
    w.safe = false;
    w.players.get_mut(&a).unwrap().character.level = 10;
    let origin = w.players[&a].pos;
    let destination = (0..w.map.height)
        .flat_map(|y| {
            (0..w.map.width).map(move |x| Pos {
                x: x as f32 + 0.5,
                y: y as f32 + 0.5,
            })
        })
        .find(|p| origin.distance(*p) > 40. && w.player_position_valid(None, *p))
        .unwrap();
    w.players.get_mut(&b).unwrap().pos = destination;
    let lich = w.spawn_entity(0x91b, origin, vec![]).unwrap();
    let ent = w.spawn_entity(0x91f, destination, vec![]).unwrap();
    (d, a, b, ar, br, lich, ent)
}

#[test]
fn quest_marker_retargets_equally_suitable_boss_after_native_teleport() {
    let (mut d, a, b, mut ar, _br, lich, ent) = quest_setup();
    let w = d.worlds.get_mut(&-2).unwrap();
    assert_eq!(w.content.objects[&0x91b].quest_level, Some(9));
    assert_eq!(w.content.objects[&0x91f].quest_level, Some(11));
    w.updates(a, false).unwrap();
    assert_eq!(w.players[&a].quest, Some(lich));
    assert!(!w.players[&a].known.contains(&ent));
    while ar.try_recv().is_ok() {}
    d.teleport(a, b, Instant::now()).unwrap();
    let w = &d.worlds[&-2];
    assert_eq!(
        w.players[&a].quest,
        Some(ent),
        "level 11 is as suitable as level 9 for a level-10 player"
    );
    assert!(w.players[&a].known.contains(&ent));
    assert!(!w.players[&a].known.contains(&lich));
    assert_eq!(d.player_view(a).unwrap().quest, Some(ent));
    let packets: Vec<_> = std::iter::from_fn(|| ar.try_recv().ok()).collect();
    let update = packets.iter().position(|p| p.id == wire::UPDATE).unwrap();
    let quest = packets.iter().position(|p| p.id == 34).unwrap();
    assert!(
        update < quest,
        "publish the new target before its quest pointer"
    );
    let mut r = Reader::new(&packets[quest].body);
    assert_eq!(r.i32().unwrap(), ent);
    r.done().unwrap();
}

#[test]
fn quest_marker_keeps_level_priority_and_excludes_unavailable_targets() {
    let (mut d, a, b, mut ar, _br, lich, ent) = quest_setup();
    let w = d.worlds.get_mut(&-2).unwrap();
    w.players.get_mut(&a).unwrap().character.level = 11;
    assert_eq!(
        w.quest_target(a),
        Some(ent),
        "closer does not override a better level match"
    );
    w.updates(a, false).unwrap();
    w.players.get_mut(&a).unwrap().character.level = 10;
    w.players.get_mut(&a).unwrap().pos = w.players[&b].pos;
    assert_eq!(w.quest_target(a), Some(ent));
    w.entities.get_mut(&ent).unwrap().published = false;
    assert_eq!(w.quest_target(a), Some(lich));
    w.entities.get_mut(&ent).unwrap().published = true;
    w.entities.get_mut(&ent).unwrap().extra_stats.push((54, 99));
    assert_eq!(w.quest_target(a), Some(lich));
    w.entities.get_mut(&ent).unwrap().extra_stats.clear();
    w.entities.get_mut(&ent).unwrap().hp = 0;
    assert_eq!(w.quest_target(a), Some(lich));
    w.entities.get_mut(&lich).unwrap().hp = 0;
    while ar.try_recv().is_ok() {}
    w.updates(a, false).unwrap();
    let packet = std::iter::from_fn(|| ar.try_recv().ok())
        .find(|p| p.id == 34)
        .unwrap();
    assert_eq!(Reader::new(&packet.body).i32().unwrap(), -1);
    w.entities.get_mut(&ent).unwrap().hp = 1;
    w.safe = true;
    assert_eq!(w.quest_target(a), None);
}

#[test]
fn selected_minimap_teleport_goto_effect_and_current_view() {
    let (mut d, a, b, mut ar, mut br) = setup();
    let info = d.worlds[&-2].map_info();
    let mut r = Reader::new(&info);
    r.i32().unwrap();
    r.i32().unwrap();
    r.utf().unwrap();
    r.i32().unwrap();
    r.i32().unwrap();
    assert_eq!(r.u8().unwrap(), 1);
    let before = d.player_view(a).unwrap();
    let receipt = d.teleport(a, b, Instant::now()).unwrap();
    let after = d.player_view(a).unwrap();
    assert!(before.position.distance(after.position) > 0.);
    assert_eq!(
        (receipt.world, receipt.player, receipt.character),
        (-2, a, 101)
    );
    assert_eq!(after.session.world, receipt.world);
    assert_eq!(after.position.distance(receipt.position), 0.);
    for rx in [&mut ar, &mut br] {
        let packets: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        let goto = packets.iter().find(|p| p.id == wire::GOTO).unwrap();
        let mut r = Reader::new(&goto.body);
        assert_eq!(r.i32().unwrap(), a);
        assert_eq!(
            (r.f32().unwrap(), r.f32().unwrap()),
            (receipt.position.x, receipt.position.y)
        );
        r.done().unwrap();
        let show = packets.iter().find(|p| p.id == wire::SHOW_EFFECT).unwrap();
        assert_eq!(show.body[0], 2);
    }
}
#[test]
fn autonomous_teleport_outputs_acknowledge_fifo_without_crossing_movement_clock_domains() {
    let (mut d, a, b, mut ar, mut br) = setup();
    let (human, mut hr) = receive(&mut d, -2, 3, WIZARD);
    let w = d.worlds.get_mut(&-2).unwrap();
    w.players.get_mut(&a).unwrap().controller = Controller::Autonomous;
    w.players.get_mut(&b).unwrap().controller = Controller::Autonomous;
    w.attack_epoch = Instant::now() - Duration::from_secs(5);
    for id in [a, b, human] {
        w.updates(id, false).unwrap();
    }
    for rx in [&mut ar, &mut br, &mut hr] {
        while rx.try_recv().is_ok() {}
    }
    let now = Instant::now();
    d.teleport(a, b, now).unwrap();
    d.teleport(b, a, now).unwrap();
    let w = d.worlds.get_mut(&-2).unwrap();
    for (id, rx) in [(a, &mut ar), (b, &mut br)] {
        assert_eq!(w.players[&id].last_move_at, 600);
        assert_eq!(w.players[&id].goto_acks.len(), 2);
        for packet in std::iter::from_fn(|| rx.try_recv().ok()) {
            if packet.id == wire::GOTO {
                w.acknowledge_autonomous_goto(id).unwrap();
            }
        }
        assert!(w.players[&id].goto_acks.is_empty());
        assert!(w.acknowledge_autonomous_goto(id).is_err());
    }
    assert_eq!(w.players[&human].goto_acks.len(), 2);
    assert!(w.acknowledge_autonomous_goto(human).is_err());
    assert_eq!(w.players[&human].goto_acks.len(), 2);
    w.now = 650;
    for id in [a, b] {
        let pos = w.players[&id].pos;
        w.move_to(
            id,
            Pos {
                x: pos.x + 0.02,
                y: pos.y,
            },
            650,
        )
        .unwrap();
        assert_eq!(w.players[&id].last_move_at, 650);
    }
    for _ in 0..2 {
        w.acknowledge_goto(human, 650).unwrap();
    }
    w.expire_goto_acks();
    for id in [a, b, human] {
        assert!(!w.players[&id].departing);
    }
}

#[test]
fn teleport_cooldown_survives_transfer_reconnect_and_refills_exactly() {
    let (mut d, a, b, _ar, _br) = setup();
    let now = Instant::now();
    d.teleport(a, b, now).unwrap();
    assert!(d.teleport(a, b, now + Duration::from_millis(9999)).is_err());
    let c = d.worlds.get_mut(&-2).unwrap().leave(a).unwrap();
    let realm = d.worlds.get_mut(&-3).unwrap();
    let a2 = realm.enter(c, None).unwrap();
    let (b2, _rx) = receive(&mut d, -3, 3, WIZARD);
    d.worlds.get_mut(&-3).unwrap().updates(a2, false).unwrap();
    assert!(
        d.teleport(a2, b2, now + Duration::from_millis(9999))
            .is_err()
    );
    assert!(d.teleport(a2, b2, now - Duration::from_millis(1)).is_err());
    d.teleport(a2, b2, now + Duration::from_secs(10)).unwrap();
    assert!(d.player_view(a).is_err());
}
#[test]
fn teleport_rejects_wrong_world_unseen_self_blocked_and_lifecycle_targets_without_charging() {
    let (mut d, a, b, _ar, _br) = setup();
    let now = Instant::now();
    let (other, _rx) = receive(&mut d, -3, 3, WIZARD);
    assert!(d.teleport(a, a, now).is_err());
    assert!(d.teleport(a, other, now).is_err());
    assert!(d.teleport(a, -999, now).is_err());
    d.worlds
        .get_mut(&-2)
        .unwrap()
        .players
        .get_mut(&a)
        .unwrap()
        .known
        .remove(&b);
    assert!(d.teleport(a, b, now).is_err());
    d.worlds.get_mut(&-2).unwrap().updates(a, false).unwrap();
    for mode in 0..6 {
        let p = d.worlds.get_mut(&-2).unwrap().players.get_mut(&b).unwrap();
        p.pending = mode == 0;
        p.death_pending = mode == 1;
        p.departing = mode == 2;
        p.persistence_failure = (mode == 3).then(|| "uncertain commit".into());
        p.character.alive = mode != 4;
        p.character.hp = if mode == 5 { 0 } else { 100 };
        assert!(d.teleport(a, b, now).is_err());
        assert!(d.teleports.is_empty());
    }
    let w = d.worlds.get_mut(&-2).unwrap();
    let p = w.players.get_mut(&b).unwrap();
    p.character.hp = 100;
    let previous = p.pos;
    p.pos = Pos { x: -1., y: -1. };
    assert!(d.teleport(a, b, now).is_err());
    d.worlds
        .get_mut(&-2)
        .unwrap()
        .players
        .get_mut(&b)
        .unwrap()
        .pos = previous;
    d.teleport(a, b, now).unwrap();
    let (a, _ar) = receive(&mut d, -1, 4, WIZARD);
    let (b, _br) = receive(&mut d, -1, 5, WIZARD);
    assert!(d.teleport(a, b, now).is_err());
}
#[test]
fn priest_heals_own_and_nearby_hp_caps_and_spends_xml_mana_with_native_effects() {
    let (mut d, a, b, mut ar, mut br) = setup();
    let w = d.worlds.get_mut(&-2).unwrap();
    w.players.get_mut(&a).unwrap().character.hp = 17;
    w.players.get_mut(&b).unwrap().character.hp = 35;
    let item = w.content.get(0xa31).unwrap().clone();
    w.activate_ability(
        b,
        Slot {
            object: b,
            slot: 1,
            kind: 0xa31,
        },
        &item,
        w.players[&b].pos,
    )
    .unwrap();
    assert_eq!(
        (
            w.players[&a].character.hp,
            w.players[&b].character.hp,
            w.players[&b].character.mp
        ),
        (100, 100, 40)
    );
    assert_eq!(d.player_view(b).unwrap().character.mp, 40);
    for rx in [&mut ar, &mut br] {
        let packets: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert_eq!(
            packets
                .iter()
                .filter(|p| p.id == wire::NOTIFICATION)
                .count(),
            2
        );
        let blast = packets
            .iter()
            .find(|p| p.id == wire::SHOW_EFFECT && p.body[0] == 5)
            .unwrap();
        let mut r = Reader::new(&blast.body);
        assert_eq!(r.u8().unwrap(), 5);
        assert_eq!(r.i32().unwrap(), b);
        assert_eq!(r.f32().unwrap(), 5.);
        assert_eq!(r.f32().unwrap(), 0.);
        assert_eq!(r.f32().unwrap(), 0.);
        assert_eq!(r.f32().unwrap(), 0.);
        assert_eq!(r.i32().unwrap() as u32, 0xffffffff);
        r.done().unwrap();
    }
}
#[test]
fn priest_range_world_lifecycle_mana_equipment_and_invalid_activations_are_strict() {
    let (mut d, a, b, _ar, _br) = setup();
    let (other, _rx) = receive(&mut d, -3, 3, WIZARD);
    d.worlds
        .get_mut(&-3)
        .unwrap()
        .players
        .get_mut(&other)
        .unwrap()
        .character
        .hp = 20;
    let w = d.worlds.get_mut(&-2).unwrap();
    let item = w.content.get(0xa31).unwrap().clone();
    let slot = Slot {
        object: b,
        slot: 1,
        kind: 0xa31,
    };
    let position = w.players[&b].pos;
    let p = w.players.get_mut(&a).unwrap();
    p.pos = Pos {
        x: position.x + 5.,
        y: position.y,
    };
    p.character.hp = 20;
    w.activate_ability(b, slot, &item, position).unwrap();
    assert_eq!(w.players[&a].character.hp, 20);
    assert_eq!(d.worlds[&-3].players[&other].character.hp, 20);
    let w = d.worlds.get_mut(&-2).unwrap();
    w.players.get_mut(&b).unwrap().character.mp = 100;
    assert!(w.activate_ability(b, slot, &item, position).is_err());
    w.now += 500;
    for mode in 0..4 {
        let p = w.players.get_mut(&a).unwrap();
        p.pos = position;
        p.death_pending = mode == 0;
        p.departing = mode == 1;
        p.pending = mode == 2;
        p.persistence_failure = (mode == 3).then(|| "failed".into());
        w.activate_ability(b, slot, &item, position).unwrap();
        assert_eq!(w.players[&a].character.hp, 20);
        w.now += 500;
        w.players.get_mut(&b).unwrap().character.mp = 100;
    }
    w.players.get_mut(&b).unwrap().character.mp = 59;
    assert!(w.activate_ability(b, slot, &item, position).is_err());
    w.players.get_mut(&b).unwrap().character.mp = 100;
    assert!(w.activate_ability(a, slot, &item, position).is_err());
    assert!(
        w.activate_ability(b, Slot { slot: 4, ..slot }, &item, position)
            .is_err()
    );
    let mut invalid = item.clone();
    invalid.activations = 2;
    assert!(w.activate_ability(b, slot, &invalid, position).is_err());
    invalid = item;
    invalid.abilities.clear();
    assert!(w.activate_ability(b, slot, &invalid, position).is_err());
    assert_eq!(w.players[&b].character.mp, 100);
}

#[test]
fn fountains_heal_only_nearby_injured_players_up_to_max_hp() {
    let mut d = super::tests::directory();
    let (a, mut ar) = receive(&mut d, -2, 1, WIZARD);
    let (b, mut br) = receive(&mut d, -2, 2, WIZARD);
    let (far, mut fr) = receive(&mut d, -2, 3, WIZARD);
    let (full, mut ur) = receive(&mut d, -2, 4, WIZARD);
    let w = d.worlds.get_mut(&-2).unwrap();
    let (fountain, pos) = *w.fountains.first().expect("Nexus map has fountains");
    let cap = w.players[&a].character.effective(&w.content).unwrap()[0];
    w.players.get_mut(&a).unwrap().pos = pos;
    w.players.get_mut(&a).unwrap().character.hp = cap - 50;
    w.players.get_mut(&b).unwrap().pos = Pos {
        x: pos.x + 4.,
        y: pos.y,
    };
    w.players.get_mut(&b).unwrap().character.hp = 10;
    w.players.get_mut(&far).unwrap().pos = Pos {
        x: pos.x + 6.,
        y: pos.y,
    };
    w.players.get_mut(&far).unwrap().character.hp = 10;
    w.players.get_mut(&full).unwrap().pos = Pos {
        x: pos.x + 2.,
        y: pos.y,
    };
    w.players.get_mut(&full).unwrap().character.hp = cap;
    for id in [a, b, far, full] {
        w.updates(id, false).unwrap();
    }
    for rx in [&mut ar, &mut br, &mut fr, &mut ur] {
        while rx.try_recv().is_ok() {}
    }
    w.now = 999;
    w.step();
    assert_eq!(w.players[&a].character.hp, cap);
    assert_eq!(w.players[&b].character.hp, cap);
    assert_eq!(w.players[&far].character.hp, 10);
    assert_eq!(w.players[&full].character.hp, cap);
    let healed: Vec<_> = std::iter::from_fn(|| ur.try_recv().ok())
        .filter(|p| p.id == wire::NOTIFICATION)
        .map(|p| Reader::new(&p.body).i32().unwrap())
        .collect();
    assert!(
        !healed.contains(&full),
        "uninjured player healed: {healed:?}"
    );
    let packets: Vec<_> = std::iter::from_fn(|| ar.try_recv().ok()).collect();
    let effects: Vec<u8> = packets
        .iter()
        .filter(|p| p.id == wire::SHOW_EFFECT)
        .map(|p| p.body[0])
        .collect();
    assert!(effects.contains(&1), "heal effect missing from {effects:?}");
    assert!(
        effects.contains(&7),
        "trail effect missing from {effects:?}"
    );
    let note = packets
        .iter()
        .find(|p| p.id == wire::NOTIFICATION)
        .expect("heal notification");
    let mut r = Reader::new(&note.body);
    assert_eq!(r.i32().unwrap(), a);
    assert_eq!(r.utf().unwrap(), "+50");
    assert_eq!(r.i32().unwrap() as u32, 0xff00ff00);
    assert!(w.players[&far].known.contains(&fountain));
}

#[test]
fn fountain_heal_waits_for_its_cadence_and_skips_sick_players() {
    let mut d = super::tests::directory();
    let (a, mut ar) = receive(&mut d, -2, 1, WIZARD);
    let w = d.worlds.get_mut(&-2).unwrap();
    let (_, pos) = *w.fountains.first().expect("Nexus map has fountains");
    let cap = w.players[&a].character.effective(&w.content).unwrap()[0];
    w.players.get_mut(&a).unwrap().pos = pos;
    w.players.get_mut(&a).unwrap().character.hp = 1;
    w.updates(a, false).unwrap();
    while ar.try_recv().is_ok() {}
    w.now = 5000;
    w.step();
    assert_eq!(w.players[&a].character.hp, cap, "fountain heals to the cap");
    // The same instant must not heal twice: fountains pulse once a second.
    w.players.get_mut(&a).unwrap().character.hp = 1;
    w.step();
    assert_eq!(w.players[&a].character.hp, 1, "cadence gate");
    // Sick players are skipped, matching the regeneration rules.
    w.now = 6000;
    w.apply_condition(a, crate::condition::SICK, 1000).unwrap();
    w.step();
    assert_eq!(w.players[&a].character.hp, 1, "sick player healed");
    w.now = 7100;
    w.step();
    assert_eq!(
        w.players[&a].character.hp, cap,
        "heals once the sickness ends"
    );
}
