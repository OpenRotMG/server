use super::*;
use crate::terrain::realms::{Source, sources};
use std::path::Path;

#[test]
fn every_catalog_realm_preserves_water_scenery_and_safe_arrivals() {
    let path = Path::new("data");
    let content = Arc::new(Content::load(path).unwrap());
    let catalog = sources(path).unwrap();
    let catalog_ids: BTreeSet<_> = catalog.iter().map(|s| s.name().to_owned()).collect();
    let mut tested = BTreeSet::new();
    let fsod = BTreeMap::from([
        (
            "realm-fsod-1",
            (
                264491,
                "7c6045ce772be1d2805a97fc4c45380d1095cdc4236ec7a2366ae0de4df80df9",
            ),
        ),
        (
            "realm-fsod-2",
            (
                246050,
                "bd1b448df6dede4a2a3ae6c36c6f47322c660e6cb1ee614175ad7853c4813f62",
            ),
        ),
        (
            "realm-fsod-3",
            (
                273177,
                "b63d971c5dc9d0d2c703903e138664565d26ff3b7131db0f2ae9686eaaf2c4ad",
            ),
        ),
    ]);
    for source in catalog {
        let Source::Encoded(asset) = &source else {
            panic!("expected production asset")
        };
        let name = asset.id.clone();
        let original = Map::decode_cells(&asset.bytes, &content).unwrap();
        if let Some(&(scenery, hash)) = fsod.get(name.as_str()) {
            use sha2::{Digest, Sha256};
            assert_eq!(format!("{:x}", Sha256::digest(&asset.bytes)), hash);
            assert_eq!(
                original.cells.iter().filter(|c| c.object.is_some()).count(),
                scenery
            );
            assert!(!original.spawns.is_empty());
        } else {
            assert!(original.spawns.is_empty());
            assert!(original.cells.iter().any(|c| c.object.is_some()));
            assert!(original.cells.iter().any(|c| !c.config.is_empty()));
            let mut habitats = [0; 13];
            for cell in &original.cells {
                habitats[cell.terrain as usize] += 1;
                assert_eq!(cell.region, 0);
                if !cell.config.is_empty() {
                    assert!(cell.object.is_some());
                    let size = cell
                        .config
                        .strip_prefix("size:")
                        .unwrap()
                        .parse::<u16>()
                        .unwrap();
                    assert!((130..=160).contains(&size) && size.is_multiple_of(5));
                }
            }
            for (terrain, count) in habitats.iter().enumerate() {
                assert!(*count > 0, "{name}: missing habitat {terrain}");
            }
        }
        let water: Vec<_> = original
            .cells
            .iter()
            .enumerate()
            .filter(|(_, c)| matches!(c.ground, 0x72 | 0x73))
            .map(|(i, c)| (i, c.ground))
            .collect();
        drop(original);
        let started = Instant::now();
        let mut directory =
            Directory::configured(path, content.clone(), 8, 12351, Some(12351), vec![source])
                .unwrap();
        let world = directory.worlds.get_mut(&-3).unwrap();
        assert_eq!(world.realm_layout.as_deref(), Some(name.as_str()));
        assert_eq!((world.map.width, world.map.height), (2048, 2048));
        assert_eq!(world.map.spawns.len(), 4);
        assert!(world.actor_ids.len() < 5000, "{name}: bounded actors");
        let hermits: Vec<_> = world
            .actors()
            .filter(|e| e.kind == crate::dungeon::trench::HERMIT)
            .collect();
        assert_eq!(hermits.len(), 1, "{name}: one original Hermit");
        assert!(world.walkable(world.ocean_reward_position(hermits[0]).unwrap()));
        assert_eq!(world.hero_identities().len(), 14);
        for kind in crate::quest::ROOTS.into_iter().chain(crate::quest::EVENTS) {
            let roots: Vec<_> = world.actors().filter(|e| e.kind == kind).collect();
            assert_eq!(roots.len(), 1, "{name}: required encounter {kind:#x}");
            let root = roots[0];
            assert_eq!(root.quest, Some(root.id));
            assert_eq!(root.hp, content.objects[&kind].hp);
            let cell = world.map.cell(root.pos.x, root.pos.y).unwrap();
            if crate::quest::EVENTS.contains(&kind) {
                assert_eq!(cell.terrain, 1);
            } else {
                assert_eq!(
                    cell.ground,
                    match kind {
                        0x91f => 0x46,
                        0x91b => 0xad,
                        0x928 => 0x60,
                        0x675 => 0xf2,
                        _ => unreachable!(),
                    }
                );
            }
        }
        assert_eq!(
            world
                .actors()
                .filter(|e| e.kind == 0xd65 && e.parent == Some(hermits[0].id))
                .count(),
            8
        );
        for (i, ground) in water {
            assert_eq!(
                world.map.cells[i].ground, ground,
                "{name}: protected water ground at {i}"
            );
        }
        for &(x, y) in &world.map.spawns {
            for y in y as i32 - 1..=y as i32 + 1 {
                for x in x as i32 - 1..=x as i32 + 1 {
                    let pos = Pos {
                        x: x as f32 + 0.5,
                        y: y as f32 + 0.5,
                    };
                    let cell = world.map.cell(pos.x, pos.y).unwrap();
                    assert_eq!((cell.terrain, cell.region, cell.object), (11, 1, None));
                    assert!(world.walkable(pos));
                }
            }
        }
        for (i, cell) in world
            .map
            .cells
            .iter()
            .enumerate()
            .filter(|(_, c)| c.object.is_some())
        {
            let pos = Pos {
                x: (i % 2048) as f32 + 0.5,
                y: (i / 2048) as f32 + 0.5,
            };
            let entity = world
                .entities_at(pos)
                .find(|e| Some(e.kind) == cell.object)
                .unwrap();
            if let Some(size) = cell.config.strip_prefix("size:") {
                assert_eq!(entity.size, size.parse::<i32>().unwrap());
            }
        }
        let layout = world.realm_layout.clone();
        let after_creation = directory.realm_rng.clone();
        let mut players = Vec::new();
        for account in 1..=4 {
            let mut character = tests::ch(world);
            character.account = account;
            character.id = account;
            let player = world.enter(character, None).unwrap();
            assert!(world.players[&player].character.alive);
            players.push(player);
        }
        assert_eq!(world.realm_layout, layout);
        use rand::RngCore;
        assert_eq!(
            directory.realm_rng.clone().next_u64(),
            after_creation.clone().next_u64()
        );
        let construction_ms = started.elapsed().as_millis();
        let started = Instant::now();
        for _ in 0..4 {
            world.step();
        }
        eprintln!(
            "{name}: construction_ms={construction_ms}, tick_mean_us={}, entities={}",
            started.elapsed().as_micros() / 4,
            world.entities.len()
        );
        assert_eq!(world.tick, 4);
        assert!(players.iter().all(|p| world.players[p].character.alive));
        let original = world.map.cells[0].region;
        let mut second = directory.make("realm-shore", "Realm", false, None).unwrap();
        assert!(!Arc::ptr_eq(&directory.worlds[&-3].map, &second.map));
        Arc::make_mut(&mut second.map).cells[0].region = 99;
        assert_eq!(directory.worlds[&-3].map.cells[0].region, original);
        tested.insert(name);
    }
    assert_eq!(tested, catalog_ids);
    assert!(fsod.keys().all(|id| tested.contains(*id)));
}

#[test]
fn spatial_queries_keep_passive_scenery_and_transient_actors_indexed() {
    let content = tests::world().content;
    let mut map = Map {
        width: 8,
        height: 8,
        spawns: vec![(4.5, 4.5)],
        cells: vec![
            crate::content::Cell {
                ground: 0xbe,
                object: None,
                config: String::new(),
                terrain: 11,
                region: 0
            };
            64
        ],
    };
    for (x, y, kind) in [(1, 1, 0x107), (3, 3, 0x600), (6, 2, 0x802)] {
        map.cells[y * 8 + x].object = Some(kind);
    }
    let mut world = World::new(content, Arc::new(map)).unwrap();
    let rock = world
        .entities
        .values()
        .find(|e| e.kind == 0x107)
        .unwrap()
        .id;
    assert!(!world.actor_ids.contains(&rock));
    assert_eq!(world.actors().count(), 2);
    assert!(!world.walkable(Pos { x: 1.5, y: 1.5 }));
    for y in 0..8 {
        for x in 0..8 {
            let pos = Pos {
                x: x as f32 + 0.5,
                y: y as f32 + 0.5,
            };
            let indexed: BTreeSet<_> = world.entities_near(pos, 2.).map(|e| e.id).collect();
            let scanned: BTreeSet<_> = world
                .entities
                .values()
                .filter(|e| e.pos.distance(pos) < 2.)
                .map(|e| e.id)
                .collect();
            assert_eq!(indexed, scanned);
        }
    }
    let transient = world
        .spawn_entity(0x107, Pos { x: 6.5, y: 6.5 }, vec![])
        .unwrap();
    assert!(world.actor_ids.contains(&transient));
    world.entities.get_mut(&transient).unwrap().expires = Some(50);
    world.now = 50;
    world.mechanic_expiry();
    assert!(!world.actor_ids.contains(&transient));
    assert!(!world.entities.contains_key(&transient));
    assert!(world.entities_at(Pos { x: 6.5, y: 6.5 }).next().is_none());
}

#[test]
fn realm_layout_selection_happens_at_creation_not_player_entry() {
    let content = tests::world().content;
    let mut directory = Directory::load(Path::new("data"), content, 8).unwrap();
    let Source::Fixture(map) = directory.realms.pop().unwrap() else {
        panic!("expected fixture")
    };
    directory.realms = (0..3)
        .map(|index| {
            let mut marked = (*map).clone();
            marked.cells[0].region = 20 + index;
            Source::Fixture(Arc::new(marked))
        })
        .collect();
    let mut rng = directory.realm_rng.clone();
    let mut selected = BTreeSet::new();
    for _ in 0..12 {
        let index = rng.gen_range(0..3usize);
        selected.insert(index);
        let mut world = directory.make("realm-shore", "Realm", false, None).unwrap();
        assert_eq!(world.map.cells[0].region, 20 + index as u8);
        let layout = world.realm_layout.clone();
        let after_creation = directory.realm_rng.clone();
        world.enter(tests::ch(&world), None).unwrap();
        assert_eq!(world.realm_layout, layout);
        use rand::RngCore;
        assert_eq!(
            directory.realm_rng.clone().next_u64(),
            after_creation.clone().next_u64()
        );
        assert_eq!(
            directory.realm_rng.clone().next_u64(),
            rng.clone().next_u64()
        );
    }
    assert_eq!(selected.len(), 3);
}

#[test]
fn catalog_order_has_reproducible_config_seeds() {
    let catalog = sources(Path::new("data")).unwrap();
    let mut seeds = BTreeMap::new();
    for seed in 0..100u64 {
        let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
        let id = catalog[rng.gen_range(0..catalog.len())].name();
        seeds.entry(id.to_owned()).or_insert(seed);
    }
    assert_eq!(
        seeds.keys().map(String::as_str).collect::<BTreeSet<_>>(),
        catalog.iter().map(|s| s.name()).collect::<BTreeSet<_>>()
    );
    for source in &catalog {
        let seed = seeds[source.name()];
        let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
        assert_eq!(
            catalog[rng.gen_range(0..catalog.len())].name(),
            source.name()
        );
        eprintln!("{}: realm_seed={seed}", source.name());
    }
}
