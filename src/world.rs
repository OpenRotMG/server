use crate::{
    content::{Content, Map, PARALYZED, Projectile, SLOWED, SPEEDY, STUNNED},
    store::{AccountLists, AccountPresentation, Character, Vault},
    wire::{self, Packet, Writer},
};
use anyhow::{Context, Result, bail, ensure};
use rand::{Rng, SeedableRng, seq::SliceRandom};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicI32, Ordering},
    },
    time::Instant,
};
use tokio::sync::{mpsc, oneshot, watch};
mod abilities;
#[cfg(test)]
mod audit_tests;
mod charges;
pub(crate) mod chat;
#[cfg(test)]
mod client_contract_tests;
mod companions;
mod consumables;
mod endgame;
mod endgame_ai;
mod enemy_damage;
mod enemy_shots;
mod families;
mod fauna;
mod fauna_births;
mod fauna_special;
mod ghost_ship;
mod gods;
#[cfg(test)]
mod gods_tests;
mod gold;
mod golems;
mod ground;
#[cfg(test)]
mod ground_tests;
mod guilds;
#[cfg(test)]
mod inventory_safety_tests;
#[cfg(test)]
mod key_tests;
mod lair;
mod landmark_ai;
#[cfg(test)]
mod landmark_tests;
mod lost_lands;
mod midlands;
#[cfg(test)]
mod midlands_tests;
mod named_births;
mod names;
mod ocean;
mod ocean_setpiece;
#[cfg(test)]
mod ocean_tests;
#[cfg(test)]
mod player_position_tests;
mod population;
#[cfg(test)]
mod population_tests;
mod realm_events;
#[cfg(test)]
mod realm_events_tests;
#[cfg(test)]
mod realm_maps_tests;
#[cfg(test)]
mod realm_movement_tests;
mod realm_portal;
mod realm_quests;
#[cfg(test)]
mod realm_quests_tests;
mod realm_setpieces;
#[cfg(test)]
mod realm_setpieces_tests;
mod spider;
#[cfg(test)]
mod spider_tests;
#[cfg(test)]
mod xp_booster_tests;
pub(crate) use chat::{chat_error, chat_info, chat_private, validate as validate_chat};
pub use endgame::ForcedTransfer;
mod abyss;
#[cfg(test)]
mod abyss_tests;
#[cfg(test)]
mod announce_tests;
#[cfg(test)]
mod bot_tests;
#[cfg(test)]
mod class_tests;
#[cfg(test)]
mod complete_abilities_tests;
#[cfg(test)]
mod consumable_tests;
mod cooperative;
#[cfg(test)]
mod cooperative_tests;
mod mechanics;
#[cfg(test)]
mod mechanics_tests;
mod native_hits;
#[cfg(test)]
mod native_hits_tests;
mod npc_attacks;
#[cfg(test)]
mod quiver_tests;
mod reconciliation;
#[cfg(test)]
mod reconciliation_tests;
mod resurrection;
pub use reconciliation::{NativeInput, RejectedNativeShot};
mod progression;
#[cfg(test)]
mod progression_tests;
pub(crate) mod shop;
mod trade;
pub(crate) mod view;
#[cfg(test)]
mod view_tests;
pub use trade::{TradeAction, TradePlan};
#[derive(Clone)]
pub struct Output {
    pub packets: mpsc::Sender<Packet>,
    pub lost: watch::Sender<bool>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Pos {
    pub x: f32,
    pub y: f32,
}
impl Pos {
    pub fn distance(self, p: Self) -> f32 {
        ((self.x - p.x).powi(2) + (self.y - p.y).powi(2)).sqrt()
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Following {
    #[default]
    Unknown,
    Acquired,
    Resting,
}
#[derive(Clone, Debug, Serialize)]
pub struct Entity {
    pub id: i32,
    pub kind: u16,
    pub name: String,
    pub pos: Pos,
    pub hp: i32,
    pub size: i32,
    pub condition: i32,
    statuses: BTreeMap<u8, (u64, u64)>,
    pub items: Vec<i32>,
    #[serde(skip)]
    drop_slots: BTreeSet<usize>,
    pub extra_stats: Vec<(u8, i32)>,
    pub reserved: bool,
    published: bool,
    damage: BTreeMap<i32, i32>,
    home: Pos,
    goal: Option<Pos>,
    #[serde(skip)]
    motion: Option<crate::encounter::Motion>,
    #[serde(skip)]
    following: Following,
    #[serde(skip)]
    charge: Option<charges::Charge>,
    #[serde(skip)]
    script: Option<landmark_ai::Script>,
    last_step: Pos,
    phase: f32,
    orbit: Option<realm_events::Orbit>,
    next_shot: u64,
    #[serde(skip)]
    attacks: crate::encounter::attacks::Clocks,
    activated: Option<u64>,
    stage: Option<u64>,
    parent: Option<i32>,
    camp: Option<usize>,
    quest: Option<i32>,
    expires: Option<u64>,
    next_aux: u64,
    blocked_since: Option<u64>,
    reaction_until: u64,
    bullet: u8,
}
impl Entity {
    fn conditions(&self, now: u64) -> i32 {
        self.condition | crate::condition::mask(&self.statuses, now)
    }
    fn accessible(&self, account: i32) -> bool {
        if !self.published {
            return false;
        }
        match self.extra_stats.iter().find(|(stat, _)| *stat == 54) {
            Some((_, owner)) if *owner >= 0 => *owner == account,
            _ => true,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Controller {
    Human,
    Qa,
    Autonomous,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AttackSuppression {
    EffectiveCooldown,
}
struct ObservedMotion {
    position: Pos,
    time: i32,
    velocity: Pos,
    at: u64,
}
pub struct Player {
    pub character: Character,
    pub pos: Pos,
    pub tx: Option<Output>,
    pub departing: bool,
    account_lists: AccountLists,
    presentation: AccountPresentation,
    paused: bool,
    pub(crate) controller: Controller,
    pub(crate) native_update_acked: bool,
    pub known: BTreeSet<i32>,
    quest: Option<i32>,
    statuses: BTreeMap<u8, (u64, u64)>,
    native_motion_condition: i32,
    known_since: BTreeMap<i32, u64>,
    shoot_acks: VecDeque<(Instant, Vec<Bullet>)>,
    weapon_ticks: VecDeque<(i32, i32)>,
    visible_projectiles: BTreeMap<(i32, u8), native_hits::VisibleProjectile>,
    ground_hits: BTreeMap<usize, i32>,
    oxygen: i32,
    tiles: BTreeSet<(i16, i16)>,
    view_tiles: std::cell::RefCell<view::Terrain>,
    pub death_started: bool,
    killer: String,
    movement_credit: f32,
    hp_fraction: f32,
    mp_fraction: f32,
    pub pending: bool,
    pub experience_queued: i32,
    clears_queued: [u64; crate::dungeon::DUNGEONS.len()],
    pub(crate) progress_started: bool,
    pub death_pending: bool,
    persistence_failure: Option<String>,
    pub bot_goal: Option<Pos>,
    pub bot_aim: Option<f32>,
    pub next_bullet: u8,
    server_bullet: u8,
    seed: u32,
    last_attack: i32,
    last_attack_at: u64,
    attack_credit: f32,
    attack_count: i32,
    attack_suppressed: Option<AttackSuppression>,
    last_move: i32,
    last_move_at: u64,
    motion: ObservedMotion,
    goto_acks: VecDeque<(bool, Instant)>,
    discarded_attack: Option<(i32, i32)>,
    tick: i32,
    last_motion: Option<(f32, u64)>,
    last_ability: Option<u64>,
    boosts: Vec<consumables::Boost>,
    last_companion: Option<u64>,
    ability_shot: Option<(u16, i32, bool, i32)>,
}
impl Player {
    fn record_motion(&mut self, position: Pos, time: i32, at: u64, maximum_speed: f32) {
        let elapsed = i64::from(time) - i64::from(self.motion.time);
        if elapsed == 0 {
            return;
        }
        let mut velocity = Pos { x: 0., y: 0. };
        if (1..=1000).contains(&elapsed) {
            velocity = Pos {
                x: (position.x - self.motion.position.x) * 1000. / elapsed as f32,
                y: (position.y - self.motion.position.y) * 1000. / elapsed as f32,
            };
            // Teleports and discontinuities are observations, not physical velocities to extrapolate.
            if velocity.x.hypot(velocity.y) > maximum_speed * 1.2 {
                velocity = Pos { x: 0., y: 0. };
            }
        }
        self.motion = ObservedMotion {
            position,
            time,
            velocity,
            at,
        };
    }
    fn quarantine(&mut self, id: i32, error: String) {
        tracing::error!(player=id,account=self.character.account,%error,"character quarantined; avatar and reservations retained");
        self.persistence_failure = Some(error);
        self.pending = true;
        self.bot_goal = None;
        self.bot_aim = None;
        if let Some(output) = &self.tx {
            let _ = output.lost.send(true);
        }
    }
    fn progress_queued(&self) -> bool {
        self.experience_queued != 0 || self.clears_queued != [0; crate::dungeon::DUNGEONS.len()]
    }
    fn active(&self) -> bool {
        self.character.alive
            && !self.death_pending
            && !self.departing
            && self.persistence_failure.is_none()
    }
    pub(crate) fn native_time(&self) -> i32 {
        self.last_move
    }
    pub(crate) fn native_transferring(&self) -> bool {
        self.departing && self.persistence_failure.is_none()
    }
    pub(crate) fn native_ready(&self, clock_paused: bool) -> bool {
        self.native_update_acked && (clock_paused || self.last_move > 0)
    }
    fn roll_damage(&mut self, spec: &Projectile) -> i32 {
        self.roll_between(spec.min, spec.max)
    }
    fn roll_between(&mut self, min: i32, max: i32) -> i32 {
        if min == max {
            min
        } else {
            self.seed = ((self.seed as u64 * 16807) % 2147483647) as u32;
            min + (self.seed % (max - min) as u32) as i32
        }
    }
    fn speed(&self, content: &Content, now: u64) -> Result<f32> {
        let condition = self.conditions(now);
        Ok(if condition & PARALYZED != 0 {
            0.
        } else if condition & SLOWED != 0 {
            0.004
        } else {
            (0.004 + (0.0096 - 0.004) * self.effective(content, now)?[4] as f32 / 75.)
                * if condition & SPEEDY != 0 { 1.5 } else { 1. }
        })
    }
}
#[derive(Clone)]
pub struct Bullet {
    pub owner: i32,
    pub source_kind: u16,
    pub id: u8,
    pub friendly: bool,
    pub start: Pos,
    pub pos: Pos,
    pub angle: f32,
    pub born: u64,
    pub damage: i32,
    pub spec: Projectile,
    pub hit: BTreeSet<i32>,
}
impl Player {
    pub(crate) fn conditions(&self, now: u64) -> i32 {
        crate::condition::mask(&self.statuses, now)
    }
}
impl Bullet {
    pub fn position(&self, now: u64) -> Pos {
        let (x, y) = self
            .spec
            .offset(self.id, self.angle, (now - self.born) as f32);
        Pos {
            x: self.start.x + x,
            y: self.start.y + y,
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Slot {
    pub object: i32,
    pub slot: usize,
    pub kind: i32,
}
#[derive(Clone, Copy)]
pub struct PortalPlan {
    locked: Option<i32>,
    opened: i32,
}
impl PortalPlan {
    fn lifetime(self) -> u64 {
        // Trapped's Create uses 20 seconds; preserve the existing Cellar's 120-second policy.
        if self.locked.is_some() { 2400 } else { 400 }
    }
}
#[derive(Clone)]
pub struct InventoryPlan {
    pub player: i32,
    pub items: Vec<i32>,
    pub revision: i32,
    pub account: i32,
    pub character: i32,
    pub bag: Option<(i32, Vec<i32>)>,
    pub effects: Vec<crate::content::ConsumableEffect>,
    pub stat_growth: Option<(Character, Slot, usize, i32)>,
    pub appearance: Option<(Slot, u32, u32)>,
    pub xp_boost: Option<Slot>,
    pub(crate) companion: Option<companions::Plan>,
    pub vault: Option<Vault>,
    pub dropped: Option<i32>,
    pub unlock: Option<PortalPlan>,
    pub collected: Option<u16>,
    pub purchase: Option<(i32, crate::store::Purchase)>,
    loot_slot: Option<(i32, usize)>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Action {
    Move { x: f32, y: f32 },
    Shoot { angle: f32 },
    Stop,
    Use { slot: usize, x: f32, y: f32 },
    Swap { from: Slot, to: Slot },
    Drop { slot: Slot },
}

struct Spawn {
    kind: u16,
    pos: Pos,
    entity: Option<i32>,
    ready: u64,
    camp: Option<usize>,
}
pub struct World {
    pub content: Arc<Content>,
    pub map: Arc<Map>,
    pub(crate) realm_layout: Option<&'static str>,
    performance: std::cell::RefCell<crate::performance::Window>,
    pub entities: BTreeMap<i32, Entity>,
    actor_ids: BTreeSet<i32>,
    pub players: BTreeMap<i32, Player>,
    pub guild: Option<(i32, i32)>,
    pub bullets: Vec<Bullet>,
    grenades: Vec<mechanics::Grenade>,
    fauna_births: Vec<fauna_births::Birth>,
    named_births: Vec<named_births::Birth>,
    lava_patches: BTreeMap<usize, (u64, u8)>,
    ability_state: abilities::State,
    companions: companions::State,
    finale: endgame_ai::Actors,
    cells: Vec<BTreeSet<i32>>,
    fountains: Vec<(i32, Pos)>,
    next_fountain_heal: u64,
    rng: rand::rngs::StdRng,
    pub now: u64,
    last_npc_step: u64,
    attack_epoch: Instant,
    clock: crate::clock::Clock,
    pub(crate) prepared_seed: Option<u64>,
    pub tick: u64,
    ids: Arc<AtomicI32>,
    pub name: String,
    pub safe: bool,
    pub owner: Option<i32>,
    pub max_players: usize,
    encounter: bool,
    population: Vec<Spawn>,
    camps: Vec<population::Camp>,
    setpieces: Vec<realm_setpieces::Area>,
    portal_requests: Vec<(u16, Pos)>,
    critical_deaths: Vec<i32>,
    admission_open: bool,
    bag_expiry: BTreeMap<i32, u64>,
    cleared: bool,
    clear_exit: Option<i32>,
    vault: Option<(i32, Vault)>,
    trades: BTreeMap<i32, trade::Trade>,
    trade_requests: BTreeMap<i32, (i32, u64)>,
}
fn allocate_id(ids: &AtomicI32) -> Result<i32> {
    let mut id = ids.load(Ordering::Relaxed);
    loop {
        let next = id
            .checked_add(1)
            .context("entity identity space exhausted")?;
        match ids.compare_exchange_weak(id, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return Ok(id),
            Err(current) => id = current,
        }
    }
}
#[derive(Clone, Copy)]
enum Target {
    World(i32),
    Player(i32),
}
struct Grant {
    account: i32,
    character: i32,
    destination: i32,
    key_time: i32,
    expires: u64,
}
struct Portal {
    origin: i32,
    destination: Option<i32>,
    expires: u64,
}
pub struct Directory {
    pub clock: crate::clock::Clock,
    pub worlds: BTreeMap<i32, World>,
    templates: BTreeMap<String, Arc<Map>>,
    realms: Vec<crate::terrain::realms::Source>,
    realm_rng: rand::rngs::StdRng,
    content: Arc<Content>,
    ids: Arc<AtomicI32>,
    grants: BTreeMap<Vec<u8>, Grant>,
    portals: BTreeMap<i32, Portal>,
    chat_budgets: BTreeMap<i32, chat::ChatBudget>,
    guild_invites: BTreeMap<i32, guilds::Invitation>,
    endgame: endgame::RealmDirectory,
    realm_portal: i32,
    realm_name: usize,
    teleports: BTreeMap<i32, Instant>,
    idle: BTreeMap<i32, u64>,
    next_world: i32,
    pub tick: u64,
    pub max_tick_us: u64,
    diagnostic_ticks: std::collections::VecDeque<(Instant, u64, u64)>,
    performance: crate::performance::Window,
    max_players: usize,
    prepared_seed: Option<u64>,
}
impl Directory {
    pub fn bug_context(&self, player: i32) -> Result<serde_json::Value> {
        let (world_id, world, p) = self
            .worlds
            .iter()
            .find_map(|(id, w)| w.players.get(&player).map(|p| (*id, w, p)))
            .context("bug player missing")?;
        let ticks = self.diagnostic_ticks.iter().filter(|(t, _, _)| t.elapsed().as_secs() <= 120).map(|(time, duration, late)| serde_json::json!({"age_ms": time.elapsed().as_millis(), "execution_us": duration, "scheduling_lateness_us": late})).collect::<Vec<_>>();
        Ok(
            serde_json::json!({"world_id": world_id, "world_name": world.name, "player_id": player,
            "character_id": p.character.id, "player_name": p.character.name, "class": p.character.class, "level": p.character.level,
            "position": {"x": p.pos.x, "y": p.pos.y}, "visible_entities": p.known.len(),
            "world_players": world.players.len(), "world_entities": world.entities.len(), "world_projectiles": world.bullets.len(),
            "native_acknowledgements": {"weapon_tick_records":p.weapon_ticks.len(),"goto":p.goto_acks.len(),"shoot":p.shoot_acks.len(),
                "oldest_goto_ms":p.goto_acks.front().map(|(_,at)|at.elapsed().as_millis()),
                "oldest_shoot_ms":p.shoot_acks.front().map(|(at,_)|at.elapsed().as_millis())},
            "tick_interval_ms": 50, "recent_ticks": ticks, "tick": self.tick,
            "total_worlds": self.worlds.len(), "total_players": self.worlds.values().map(|w| w.players.len()).sum::<usize>(),
            "total_entities": self.worlds.values().map(|w| w.entities.len()).sum::<usize>(),
            "total_projectiles": self.worlds.values().map(|w| w.bullets.len()).sum::<usize>()}),
        )
    }
    pub fn announce(&self, text: &str) -> usize {
        self.worlds
            .values()
            .map(|w| w.broadcast_text(chat::ANNOUNCEMENT, text))
            .sum()
    }
    pub fn competitive_characters(&self) -> Vec<Character> {
        self.worlds
            .values()
            .flat_map(|w| w.players.values())
            .filter(|p| p.controller == Controller::Human && p.persistence_failure.is_none())
            .map(|p| p.character.clone())
            .collect()
    }
    pub fn health(&self, started: Instant) -> serde_json::Value {
        let players: usize = self.worlds.values().map(|w| w.players.len()).sum();
        let qa_actors: usize = self
            .worlds
            .values()
            .flat_map(|w| w.players.values())
            .filter(|p| p.controller == Controller::Qa)
            .count();
        let bots = self
            .worlds
            .values()
            .flat_map(|w| w.players.values())
            .filter(|p| p.controller == Controller::Autonomous)
            .count();
        serde_json::json!({
            "status": "development", "uptime_seconds": started.elapsed().as_secs(),
            "worlds": self.worlds.len(), "players": players - qa_actors - bots, "bots": bots, "qa_actors": qa_actors,
            "entities": self.worlds.values().map(|w| w.entities.len()).sum::<usize>(),
            "projectiles": self.worlds.values().map(|w| w.bullets.len()).sum::<usize>(),
            "tick": self.tick, "max_tick_us": self.max_tick_us,
        })
    }
    #[cfg(test)]
    pub fn load(path: &std::path::Path, content: Arc<Content>, max_players: usize) -> Result<Self> {
        Self::test_with_seed(path, content, max_players, 0)
    }
    #[cfg(test)]
    pub(crate) fn test_with_seed(
        path: &std::path::Path,
        content: Arc<Content>,
        max_players: usize,
        seed: u64,
    ) -> Result<Self> {
        Self::fixture(path, content, max_players, seed, None)
    }
    #[cfg(test)]
    pub(crate) fn test_prepared(
        path: &std::path::Path,
        content: Arc<Content>,
        max_players: usize,
        seed: u64,
    ) -> Result<Self> {
        Self::fixture(path, content, max_players, seed, Some(seed))
    }
    #[cfg(test)]
    fn fixture(
        path: &std::path::Path,
        content: Arc<Content>,
        max_players: usize,
        seed: u64,
        prepared_seed: Option<u64>,
    ) -> Result<Self> {
        let map = Arc::new(crate::terrain::generate(&content, seed, 512)?);
        Self::configured(
            path,
            content,
            max_players,
            seed,
            prepared_seed,
            vec![crate::terrain::realms::Source::Fixture(map)],
        )
    }
    pub fn with_seed(
        path: &std::path::Path,
        content: Arc<Content>,
        max_players: usize,
        seed: u64,
    ) -> Result<Self> {
        Self::configured(
            path,
            content,
            max_players,
            seed,
            None,
            crate::terrain::realms::sources(path)?,
        )
    }
    pub(crate) fn prepared(
        path: &std::path::Path,
        content: Arc<Content>,
        max_players: usize,
        seed: u64,
    ) -> Result<Self> {
        Self::configured(
            path,
            content,
            max_players,
            seed,
            Some(seed),
            crate::terrain::realms::sources(path)?,
        )
    }
    fn configured(
        path: &std::path::Path,
        content: Arc<Content>,
        max_players: usize,
        seed: u64,
        prepared_seed: Option<u64>,
        realms: Vec<crate::terrain::realms::Source>,
    ) -> Result<Self> {
        ensure!(!realms.is_empty(), "Realm layout pool is empty");
        crate::endgame::validate(&content)?;
        let mut templates = BTreeMap::new();
        for name in [
            "tutorial",
            "nexus",
            "nexus-limbo",
            "vault",
            "pirate-cave",
            crate::endgame::CASTLE,
            crate::endgame::CHAMBER,
            crate::endgame::CELLAR,
            "sprite-world",
            "undead-lair",
            "guild-hall-1",
            "guild-hall-2",
            "guild-hall-3",
            "guild-hall-4",
        ] {
            templates.insert(
                name.to_string(),
                Arc::new(Map::load(&path.join(format!("{name}.wmap")), &content)?),
            );
        }
        let mut d = Self {
            clock: crate::clock::Clock::default(),
            worlds: BTreeMap::new(),
            templates,
            realms,
            realm_rng: rand::rngs::StdRng::seed_from_u64(seed),
            content,
            ids: Arc::new(AtomicI32::new(1000)),
            grants: BTreeMap::new(),
            portals: BTreeMap::new(),
            chat_budgets: BTreeMap::new(),
            guild_invites: BTreeMap::new(),
            endgame: endgame::RealmDirectory::default(),
            realm_portal: 0,
            realm_name: (seed % realm_portal::NAMES.len() as u64) as usize,
            teleports: BTreeMap::new(),
            idle: BTreeMap::new(),
            next_world: 1,
            tick: 0,
            max_tick_us: 0,
            diagnostic_ticks: std::collections::VecDeque::new(),
            performance: crate::performance::Window::default(),
            max_players,
            prepared_seed,
        };
        for (id, template, name, safe) in [
            (-1, "tutorial", "Tutorial", false),
            (-2, "nexus", "Nexus", true),
            (-3, "realm-shore", "Realm", false),
            (-4, "nexus-limbo", "Nexus Limbo", true),
        ] {
            let world = d.make(template, name, safe, None)?;
            d.worlds.insert(id, world);
        }
        d.bind_realm_heroes(-3, d.worlds[&-3].hero_identities())?;
        let nexus = d.worlds.get_mut(&-2).expect("constructed Nexus");
        let pos = nexus
            .map
            .cells
            .iter()
            .enumerate()
            .find_map(|(i, c)| {
                let pos = Pos {
                    x: (i % nexus.map.width as usize) as f32 + 0.5,
                    y: (i / nexus.map.width as usize) as f32 + 0.5,
                };
                (c.region == 2 && c.object.is_none() && nexus.walkable(pos)).then_some(pos)
            })
            .context("Nexus has no walkable Realm portal region")?;
        d.realm_portal = nexus.spawn_entity(0x712, pos, vec![])?;
        d.sync_realm_portal();
        Ok(d)
    }
    fn make(
        &mut self,
        template: &str,
        name: &str,
        safe: bool,
        owner: Option<i32>,
    ) -> Result<World> {
        let layout =
            (template == "realm-shore").then(|| self.realm_rng.gen_range(0..self.realms.len()));
        let mut world = World::with_ids(
            self.content.clone(),
            if let Some(index) = layout {
                self.realms[index].open(&self.content)?
            } else if template == "abyss-of-demons" {
                let seed = match self.prepared_seed {
                    Some(seed) => seed ^ self.next_world as u64,
                    None => rand::random(),
                };
                Arc::new(crate::dungeon::abyss::generate(&self.content, seed)?)
            } else if template == "ocean-trench" {
                let seed = self
                    .prepared_seed
                    .map_or_else(rand::random, |s| s ^ self.next_world as u64);
                Arc::new(crate::dungeon::trench::generate(&self.content, seed)?)
            } else if template == "spider-den" {
                let seed = match self.prepared_seed {
                    Some(seed) => seed ^ self.next_world as u64,
                    None => rand::random(),
                };
                Arc::new(crate::dungeon::spider::generate(&self.content, seed)?)
            } else if template == "snake-pit" {
                Arc::new(crate::dungeon::snake_pit(&self.content, rand::random())?)
            } else {
                self.templates
                    .get(template)
                    .context("world template missing")?
                    .clone()
            },
            self.ids.clone(),
        )?;
        world.realm_layout = layout.map(|index| self.realms[index].name());
        world.clock = self.clock.clone();
        world.attack_epoch = self.clock.now();
        if let Some(seed) = self.prepared_seed {
            let seed = seed ^ self.next_world as u64;
            world.rng = rand::rngs::StdRng::seed_from_u64(seed);
            world.prepared_seed = Some(seed);
        }
        world.name = name.into();
        world.safe = safe;
        world.owner = owner;
        world.max_players = self.max_players;
        world.encounter = template == "realm-shore"
            || template == crate::endgame::CASTLE
            || crate::dungeon::DUNGEONS
                .iter()
                .chain(crate::endgame::DUNGEONS)
                .any(|(_, name, _, _)| *name == template);
        if template == "realm-shore" {
            world.populate_shore()?;
            world.populate_inland()?;
            world.populate_realm_heroes()?;
            world.populate_named_quests()?;
            world.populate_realm_events()?;
            world.populate_hermit()?;
            tracing::info!(
                layout = world.realm_layout,
                entities = world.entities.len(),
                actors = world.actor_ids.len(),
                "Realm simulation populated"
            );
        }
        world.prepare_endgame()?;
        if template == "sprite-world" {
            let exits: Vec<_> = world
                .entities
                .values()
                .filter(|e| e.kind == 0x70e)
                .map(|e| e.id)
                .collect();
            ensure!(
                exits.len() == 1,
                "Sprite World requires one authored completion exit"
            );
            world.entities.get_mut(&exits[0]).unwrap().published = false;
            world.clear_exit = Some(exits[0]);
        }
        if world.encounter {
            let kinds: BTreeSet<_> = world
                .entities
                .values()
                .filter(|e| {
                    let object = &world.content.objects[&e.kind];
                    object.enemy && object.class != "Wall"
                })
                .map(|e| e.kind)
                .collect();
            for kind in kinds {
                crate::encounter::validate(&world.content, kind)?;
                crate::loot::validate(&world.content, kind)?;
            }
        }
        for kind in [
            0x173, 0x653, 0x918, 0x919, 0xd07, 0xd08, 0xd09, 0xd0a, 0xdaa, 0xdab, 0xdac, 0xdad,
            0xdae, 0xdaf, 0xdb0, 0xdb1, 0xdb2,
        ] {
            crate::encounter::validate(&world.content, kind)?;
            crate::loot::validate(&world.content, kind)?;
        }
        Ok(world)
    }
    fn resolve(&mut self, target: Target) -> Result<&mut World> {
        match target {
            Target::World(id) => self.worlds.get_mut(&id).context("world no longer exists"),
            Target::Player(id) => self
                .worlds
                .values_mut()
                .find(|w| w.players.contains_key(&id))
                .context("player missing from world directory"),
        }
    }
    pub(crate) fn character_transferring(&self, character: i32) -> bool {
        self.grants.values().any(|g| g.character == character)
    }
    pub fn location(&self, player: i32) -> Result<i32> {
        self.worlds
            .iter()
            .find(|(_, w)| w.players.contains_key(&player))
            .map(|(id, _)| *id)
            .context("player missing from world directory")
    }
    pub fn initial(&self, game: i32) -> Result<i32> {
        let destination = match game {
            0 | -2 => -2,
            -1 => -1,
            -3 => self.endgame.active,
            -4 => -4,
            _ => bail!("world entry {game} requires a transfer grant"),
        };
        ensure!(
            self.worlds.contains_key(&destination),
            "entry world missing"
        );
        ensure!(self.worlds[&destination].admission_open, "Realm is closed");
        Ok(destination)
    }
    pub(crate) fn prepared_instance(&mut self, template: &str) -> Result<i32> {
        ensure!(
            self.prepared_seed.is_some(),
            "prepared encounter requires isolated test state"
        );
        if template != crate::endgame::CASTLE {
            return self.instance(template, None);
        }
        ensure!(self.worlds.len() < 12, "world instance limit reached");
        let world = self.make(template, "Oryx's Castle", false, None)?;
        let id = self.next_world;
        self.next_world = id
            .checked_add(1)
            .context("world identity space exhausted")?;
        self.worlds.insert(id, world);
        self.idle.insert(id, self.tick);
        Ok(id)
    }
    pub fn instance(&mut self, template: &str, account: Option<i32>) -> Result<i32> {
        ensure!(
            matches!((template, account), ("vault", Some(_)))
                || (account.is_none()
                    && crate::dungeon::DUNGEONS
                        .iter()
                        .chain(crate::endgame::DUNGEONS)
                        .any(|(_, name, _, _)| *name == template)),
            "invalid private/dungeon instance policy"
        );
        if let Some(account) = account
            && let Some((id, _)) = self.worlds.iter().find(|(_, w)| w.owner == Some(account))
        {
            return Ok(*id);
        }
        ensure!(self.worlds.len() < 12, "world instance limit reached");
        let world = self.make(
            template,
            if template == "vault" {
                "Vault"
            } else {
                crate::dungeon::DUNGEONS
                    .iter()
                    .chain(crate::endgame::DUNGEONS)
                    .find(|(_, name, _, _)| *name == template)
                    .context("dungeon registration missing")?
                    .2
            },
            template == "vault",
            account,
        )?;
        let id = self.next_world;
        self.next_world = id
            .checked_add(1)
            .context("world identity space exhausted")?;
        self.worlds.insert(id, world);
        self.idle.insert(id, self.tick);
        tracing::info!(world = id, template, account, "world instance created");
        Ok(id)
    }
    pub fn destination(&mut self, player: i32, portal: Option<i32>) -> Result<(i32, i32, i32)> {
        let origin = self.location(player)?;
        let world = &self.worlds[&origin];
        let p = &world.players[&player];
        ensure!(
            p.character.alive && p.character.hp > 0 && !p.death_pending && !p.departing,
            "character cannot transfer"
        );
        let (account, character) = (p.character.account, p.character.id);
        let kind = if let Some(id) = portal {
            let e = world.entities.get(&id).context("portal missing")?;
            ensure!(e.published, "portal not published");
            ensure!(p.pos.distance(e.pos) <= 2., "portal out of reach");
            e.kind
        } else {
            0x712
        };
        let destination = match kind {
            0x712 if origin == -2 && portal == Some(self.realm_portal) => self.initial(-3)?,
            0x703 | 0x70d | 0x712 | 0x71d => -2,
            0x704 | 0x70e | 0x71c => self.initial(-3)?,
            0x720 => self.instance("vault", Some(account))?,
            0x72f => {
                let guild = p
                    .presentation
                    .guild
                    .as_ref()
                    .context("You are not in a guild.")?
                    .id;
                *self
                    .worlds
                    .iter()
                    .find(|(_, w)| w.guild.is_some_and(|(id, _)| id == guild))
                    .context("Guild Hall has not been initialized.")?
                    .0
            }
            _ if portal.is_some_and(|id| self.portals.contains_key(&id)) => {
                let binding = self
                    .portals
                    .get(&portal.context("dungeon portal identity missing")?)
                    .context("dropped portal has no live instance binding")?;
                ensure!(
                    binding.origin == origin && binding.expires > self.tick,
                    "dungeon portal binding expired or belongs to another world"
                );
                binding
                    .destination
                    .context("This dungeon is not implemented yet.")?
            }
            _ => bail!("unsupported portal destination {kind:#x}"),
        };
        ensure!(destination != origin, "already in destination world");
        Ok((account, character, destination))
    }
    pub fn issue(
        &mut self,
        account: i32,
        character: i32,
        destination: i32,
    ) -> Result<(i32, Vec<u8>, String)> {
        ensure!(self.grants.len() < 64, "transfer grant limit reached");
        let world = self
            .worlds
            .get(&destination)
            .context("transfer destination missing")?;
        ensure!(
            world.owner.is_none_or(|owner| owner == account),
            "private destination belongs to another account"
        );
        ensure!(world.admission_open, "Realm is closed");
        let name = world.name.clone();
        let mut key = vec![0; 16];
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut key);
        let key_time = i32::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs(),
        )?;
        self.grants.insert(
            key.clone(),
            Grant {
                account,
                character,
                destination,
                key_time,
                expires: self.tick + 2400,
            },
        );
        tracing::info!(
            account,
            character,
            destination,
            expires_tick = self.tick + 2400,
            "transfer grant issued"
        );
        Ok((key_time, key, name))
    }
    pub fn consume(
        &mut self,
        account: i32,
        destination: i32,
        key_time: i32,
        key: Vec<u8>,
    ) -> Result<i32> {
        let grant = self
            .grants
            .get(&key)
            .context("unknown or already used transfer grant")?;
        ensure!(grant.expires > self.tick, "transfer grant expired");
        ensure!(
            grant.account == account
                && grant.destination == destination
                && grant.key_time == key_time,
            "transfer grant binding mismatch"
        );
        ensure!(
            self.worlds.contains_key(&destination),
            "transfer destination expired"
        );
        ensure!(self.worlds[&destination].admission_open, "Realm is closed");
        let character = grant.character;
        self.grants.remove(&key);
        if destination > 0 {
            self.idle.insert(destination, self.tick);
        }
        tracing::info!(account, character, destination, "transfer grant consumed");
        Ok(character)
    }
    #[cfg(test)]
    fn drop_portal(&mut self, origin: i32, pos: Pos) -> Result<i32> {
        self.drop_dungeon_portal(origin, 0x717, pos)
    }
    fn drop_dungeon_portal(&mut self, origin: i32, kind: u16, pos: Pos) -> Result<i32> {
        let definition = crate::dungeon::DUNGEONS
            .iter()
            .find(|(portal, _, _, _)| *portal == kind)
            .context("unregistered earned dungeon portal")?;
        ensure!(
            self.worlds[&origin].name == "Realm" && self.worlds[&origin].walkable(pos),
            "dungeon portal must be earned on the walkable Realm"
        );
        let destination = self.instance(definition.1, None)?;
        let id = self
            .worlds
            .get_mut(&origin)
            .unwrap()
            .spawn_entity(kind, pos, vec![])?;
        self.portals.insert(
            id,
            Portal {
                origin,
                destination: Some(destination),
                // The May 2012 Sprite World guide specifies 15 seconds, unlike newer references.
                expires: self.tick + if kind == 0x70c { 300 } else { 600 },
            },
        );
        tracing::info!(
            portal = id,
            kind,
            origin,
            destination,
            "earned dungeon portal dropped"
        );
        Ok(id)
    }
    fn step(&mut self) {
        self.tick += 1;
        self.sync_realm_portal();
        self.grants.retain(|_, grant| grant.expires > self.tick);
        for (id, world) in &mut self.worlds {
            world.step();
            if *id > 0 && *id != self.endgame.active {
                if world.players.is_empty() {
                    self.idle.entry(*id).or_insert(self.tick);
                } else {
                    self.idle.remove(id);
                }
            }
        }
        self.endgame_step();
        let requests: Vec<_> = self
            .worlds
            .iter_mut()
            .flat_map(|(id, world)| {
                std::mem::take(&mut world.portal_requests)
                    .into_iter()
                    .map(|pos| (*id, pos))
            })
            .collect();
        for (origin, (kind, pos)) in requests {
            if let Err(error) = self.drop_dungeon_portal(origin, kind, pos) {
                tracing::error!(origin,%error,"earned dungeon portal creation failed");
            }
        }
        let expired_portals: Vec<_> = self
            .portals
            .iter()
            .filter(|(id, binding)| {
                binding.expires <= self.tick
                    && self
                        .worlds
                        .get(&binding.origin)
                        .and_then(|w| w.entities.get(id))
                        .is_none_or(|e| !e.reserved)
            })
            .map(|(id, binding)| (*id, binding.origin))
            .collect();
        for (id, origin) in expired_portals {
            self.worlds
                .get_mut(&origin)
                .expect("portal origin exists")
                .remove_entity(id);
            self.portals.remove(&id);
            tracing::info!(portal = id, origin, "dungeon portal expired");
        }
        let expired: Vec<_> = self
            .idle
            .iter()
            .filter(|(id, since)| {
                self.tick - **since > 6000
                    && !self.grants.values().any(|grant| grant.destination == **id)
                    && !self
                        .portals
                        .values()
                        .any(|portal| portal.destination == Some(**id))
            })
            .map(|(id, _)| *id)
            .collect();
        for id in expired {
            self.worlds.remove(&id);
            self.idle.remove(&id);
            tracing::info!(world = id, "idle world removed");
        }
    }
}
type Job = Box<dyn FnOnce(&mut Directory) + Send>;
#[derive(Clone)]
pub struct Handle {
    tx: mpsc::Sender<Job>,
    target: Target,
    pub clock: crate::clock::Clock,
}
impl Handle {
    pub fn spawn(directory: Directory) -> Self {
        let (tx, mut rx) = mpsc::channel::<Job>(256);
        let clock = directory.clock.clone();
        tokio::spawn(async move {
            let mut directory = directory;
            let mut timer = tokio::time::interval(std::time::Duration::from_millis(50));
            timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    scheduled = timer.tick() => {
                        if directory.clock.paused() { continue; }
                        let late=scheduled.elapsed().as_micros() as u64;
                        let start=Instant::now(); directory.step();
                        let us=start.elapsed().as_micros() as u64;
                        directory.max_tick_us=directory.max_tick_us.max(us);
                        directory.performance.record("step", start.elapsed());
                        directory.performance.record("scheduling_lateness", std::time::Duration::from_micros(late));
                        directory.diagnostic_ticks.push_back((start, us, late));
                        while directory.diagnostic_ticks.front().is_some_and(|(t, _, _)| t.elapsed().as_secs() > 120) { directory.diagnostic_ticks.pop_front(); }
                        if us>25_000 {tracing::warn!(tick=directory.tick,elapsed_us=us,"slow aggregate simulation tick");}
                        if directory.tick.is_multiple_of(100) { directory.max_tick_us=0; }
                        if let Some(timings)=directory.performance.report() {
                            tracing::info!(tick=directory.tick,worlds=directory.worlds.len(),
                                players=directory.worlds.values().map(|w|w.players.len()).sum::<usize>(),
                                entities=directory.worlds.values().map(|w|w.entities.len()).sum::<usize>(),
                                actors=directory.worlds.values().map(|w|w.actor_ids.len()).sum::<usize>(),
                                diagnostic_tick_samples=directory.diagnostic_ticks.len(),queued_jobs=rx.len(),
                                memory=%crate::performance::memory(),timings=%timings,"directory health");
                        }
                    },
                    job=rx.recv()=>match job{Some(f)=>f(&mut directory),None=>break}
                }
            }
        });
        Self {
            tx,
            target: Target::World(-1),
            clock,
        }
    }
    pub fn in_world(&self, id: i32) -> Self {
        Self {
            tx: self.tx.clone(),
            target: Target::World(id),
            clock: self.clock.clone(),
        }
    }
    pub fn for_player(&self, id: i32) -> Self {
        Self {
            tx: self.tx.clone(),
            target: Target::Player(id),
            clock: self.clock.clone(),
        }
    }
    pub async fn directory<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Directory) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let (tx, rx) = oneshot::channel();
        let queued = Instant::now();
        self.tx
            .send(Box::new(move |d| {
                d.performance.record("job_wait", queued.elapsed());
                let started = Instant::now();
                let result = f(d);
                d.performance.record("job", started.elapsed());
                let _ = tx.send(result);
            }))
            .await
            .map_err(|_| anyhow::anyhow!("world directory stopped"))?;
        rx.await.context("world directory worker failed")?
    }
    pub async fn collect<T: Send + 'static>(
        &self,
        mut f: impl FnMut(&mut World) -> Vec<T> + Send + 'static,
    ) -> Result<Vec<T>> {
        self.directory(move |d| Ok(d.worlds.values_mut().flat_map(&mut f).collect()))
            .await
    }
    pub async fn call<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut World) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let target = self.target;
        self.directory(move |d| f(d.resolve(target)?)).await
    }
}
pub(crate) fn send(p: &Player, id: u8, body: Vec<u8>) {
    if let Some(tx) = &p.tx
        && tx.packets.try_send(Packet { id, body }).is_err()
    {
        tx.lost.send_replace(true);
        tracing::warn!(
            character = p.character.id,
            "slow/closed connection; packet queue full"
        );
    }
}
fn stat(out: &mut Writer, id: u8, v: i32) {
    out.u8(id).i32(v);
}
fn text_stat(out: &mut Writer, id: u8, v: &str) {
    out.u8(id).utf(v);
}
impl World {
    #[cfg(test)]
    pub fn new(content: Arc<Content>, map: Arc<Map>) -> Result<Self> {
        Self::with_ids(content, map, Arc::new(AtomicI32::new(1000)))
    }
    fn with_ids(content: Arc<Content>, map: Arc<Map>, ids: Arc<AtomicI32>) -> Result<Self> {
        let mut entities = BTreeMap::new();
        let mut actor_ids = BTreeSet::new();
        let mut fountains = Vec::new();
        let mut cells = vec![BTreeSet::new(); map.cells.len()];
        for (y, row) in map.cells.chunks(map.width as usize).enumerate() {
            for (x, c) in row.iter().enumerate() {
                if let Some(kind) = c.object {
                    let id = allocate_id(&ids)?;
                    let o = content.get(kind)?;
                    let pos = Pos {
                        x: x as f32 + 0.5,
                        y: y as f32 + 0.5,
                    };
                    let mut e = Entity {
                        id,
                        kind,
                        name: if o.class == "Portal" {
                            o.name.clone()
                        } else {
                            String::new()
                        },
                        pos,
                        home: pos,
                        goal: None,
                        motion: None,
                        following: Following::Unknown,
                        charge: None,
                        script: None,
                        last_step: Pos { x: 0., y: 0. },
                        phase: 0.,
                        orbit: None,
                        hp: o.hp,
                        size: o.size,
                        condition: crate::dungeon::abyss::initial_condition(kind)
                            | crate::dungeon::spider::initial_condition(kind)
                            | if o.stasis_immune {
                                crate::condition::bit(crate::condition::STASIS_IMMUNE)
                            } else {
                                0
                            },
                        statuses: BTreeMap::new(),
                        items: o.equipment.clone(),
                        drop_slots: BTreeSet::new(),
                        extra_stats: if o.class == "Portal" {
                            vec![(37, 1)]
                        } else {
                            vec![]
                        },
                        reserved: false,
                        published: true,
                        damage: BTreeMap::new(),
                        next_shot: 0,
                        attacks: Default::default(),
                        activated: None,
                        stage: None,
                        parent: None,
                        camp: None,
                        quest: None,
                        expires: None,
                        next_aux: 0,
                        blocked_since: None,
                        reaction_until: 0,
                        bullet: 0,
                    };
                    for setting in c.config.split(';').filter(|s| !s.is_empty()) {
                        let (k, v) = setting.split_once(':').context("malformed map setting")?;
                        match k {
                            "name" => e.name = v.into(),
                            "size" => e.size = v.parse()?,
                            "eff" => {
                                e.condition |= i32::from_str_radix(v.trim_start_matches("0x"), 16)?
                            }
                            "conn" | "mtype" | "mcount" | "mtime" | "nstar" => {
                                let value = if let Some(hex) = v.strip_prefix("0x") {
                                    u32::from_str_radix(hex, 16)? as i32
                                } else {
                                    v.parse()?
                                };
                                let stat = match k {
                                    "conn" => 41,
                                    "mtype" => 34,
                                    "mcount" => 42,
                                    "mtime" => 43,
                                    // NameChanger is not a SellableObject in 123.5.1.
                                    "nstar" => 55,
                                    _ => unreachable!(),
                                };
                                e.extra_stats.push((stat, value));
                            }
                            _ => bail!("unsupported map setting {k}"),
                        }
                    }
                    if o.class == "Merchant" {
                        shop::configure(&mut e, &content)?;
                    }
                    if o.name == "White Fountain" {
                        fountains.push((id, pos));
                    }
                    cells[y * map.width as usize + x].insert(id);
                    if o.enemy
                        || o.quest
                        || !o.static_object
                        || matches!(kind, 0x173 | 0xd7d | 0x802..=0x805)
                    {
                        actor_ids.insert(id);
                    }
                    entities.insert(id, e);
                }
            }
        }
        Ok(Self {
            content,
            map,
            realm_layout: None,
            entities,
            actor_ids,
            players: BTreeMap::new(),
            guild: None,
            bullets: Vec::new(),
            grenades: Vec::new(),
            fauna_births: Vec::new(),
            named_births: Vec::new(),
            lava_patches: BTreeMap::new(),
            ability_state: abilities::State::default(),
            companions: companions::State::default(),
            finale: endgame_ai::Actors::default(),
            cells,
            fountains,
            next_fountain_heal: 0,
            rng: rand::rngs::StdRng::from_entropy(),
            now: 0,
            last_npc_step: 0,
            performance: std::cell::RefCell::default(),
            attack_epoch: Instant::now(),
            clock: crate::clock::Clock::default(),
            prepared_seed: None,
            tick: 0,
            ids,
            name: "Development tutorial".into(),
            safe: false,
            owner: None,
            max_players: 85,
            encounter: false,
            population: vec![],
            camps: vec![],
            setpieces: vec![],
            portal_requests: vec![],
            critical_deaths: vec![],
            admission_open: true,
            bag_expiry: BTreeMap::new(),
            cleared: false,
            clear_exit: None,
            vault: None,
            trades: BTreeMap::new(),
            trade_requests: BTreeMap::new(),
        })
    }
    pub fn settled(&self) -> Result<bool> {
        for p in self.players.values() {
            ensure!(
                p.persistence_failure.is_none(),
                "character {} is quarantined",
                p.character.id
            );
        }
        Ok(self.players.values().all(|p| {
            !p.pending
                && !p.progress_queued()
                && !p.progress_started
                && !p.death_pending
                && !p.departing
        }))
    }
    pub fn load_vault(&mut self, vault: Vault) -> Result<()> {
        ensure!(
            self.name == "Vault" && self.owner == Some(vault.account),
            "vault world owner mismatch"
        );
        ensure!(
            vault.items.len() == 8,
            "vault must contain exactly eight slots"
        );
        if let Some((id, loaded)) = &self.vault {
            ensure!(
                loaded.revision == vault.revision && self.entities[id].items == vault.items,
                "loaded vault disagrees with durable revision"
            );
            return Ok(());
        }
        let (x, y) = self.map.spawns[0];
        let spawn = Pos { x, y };
        let pos = self
            .map
            .cells
            .iter()
            .enumerate()
            .filter(|(_, c)| c.region == 12)
            .map(|(i, _)| Pos {
                x: (i % self.map.width as usize) as f32 + 0.5,
                y: (i / self.map.width as usize) as f32 + 0.5,
            })
            .min_by(|a, b| a.distance(spawn).total_cmp(&b.distance(spawn)))
            .context("sourced Vault has no chest region")?;
        let id = self.spawn_entity(0x504, pos, vault.items.clone())?;
        self.entities
            .get_mut(&id)
            .unwrap()
            .extra_stats
            .push((54, vault.account));
        tracing::info!(
            entity = id,
            account = vault.account,
            revision = vault.revision,
            "private eight-slot chest loaded"
        );
        self.vault = Some((id, vault));
        Ok(())
    }
    pub fn spawn_entity(&mut self, kind: u16, pos: Pos, items: Vec<i32>) -> Result<i32> {
        ensure!(self.walkable(pos), "entity spawn is blocked");
        self.insert_entity(kind, pos, items)
    }
    fn insert_entity(&mut self, kind: u16, pos: Pos, items: Vec<i32>) -> Result<i32> {
        ensure!(
            pos.x.is_finite() && pos.y.is_finite() && self.map.cell(pos.x, pos.y).is_some(),
            "entity spawn outside map"
        );
        let cell = pos.y.floor() as usize * self.map.width as usize + pos.x.floor() as usize;
        let definition = self.content.get(kind)?;
        let id = allocate_id(&self.ids)?;
        let extra_stats = if definition.class == "Portal" {
            vec![(37, 1)]
        } else {
            vec![]
        };
        self.entities.insert(
            id,
            Entity {
                id,
                kind,
                name: if definition.class == "Portal" {
                    definition.name.clone()
                } else {
                    String::new()
                },
                pos,
                home: pos,
                goal: None,
                motion: None,
                following: Following::Unknown,
                charge: None,
                script: None,
                last_step: Pos { x: 0., y: 0. },
                phase: 0.,
                orbit: None,
                hp: definition.hp,
                size: definition.size,
                condition: crate::dungeon::abyss::initial_condition(kind)
                    | crate::dungeon::spider::initial_condition(kind)
                    | (if kind == 0x91a {
                        crate::condition::bit(crate::condition::INVULNERABLE)
                    } else {
                        0
                    })
                    | (if definition.stasis_immune {
                        crate::condition::bit(crate::condition::STASIS_IMMUNE)
                    } else {
                        0
                    }),

                statuses: BTreeMap::new(),
                items,
                drop_slots: BTreeSet::new(),
                extra_stats,
                reserved: false,
                published: true,
                damage: BTreeMap::new(),
                next_shot: 0,
                attacks: Default::default(),
                activated: None,
                stage: None,
                parent: None,
                camp: None,
                quest: None,
                expires: None,
                next_aux: 0,
                blocked_since: None,
                reaction_until: 0,
                bullet: 0,
            },
        );
        self.cells[cell].insert(id);
        self.actor_ids.insert(id);
        Ok(id)
    }
    fn spawn_exit(&mut self, kind: u16, loot: Pos) -> Result<i32> {
        // Native interaction picks one nearest object; an exit on a bag makes that loot unreachable.
        let pos = (0..8)
            .map(|i| {
                let angle = i as f32 * std::f32::consts::TAU / 8.;
                Pos {
                    x: loot.x + 2. * angle.cos(),
                    y: loot.y + 2. * angle.sin(),
                }
            })
            .find(|pos| {
                self.walkable(*pos)
                    && self
                        .entities
                        .values()
                        .filter(|e| {
                            matches!(
                                self.content.objects[&e.kind].class.as_str(),
                                "Portal" | "Container"
                            )
                        })
                        .all(|e| e.pos.distance(*pos) > 1.5)
            })
            .context("boss exit has no walkable position clear of loot and interactions")?;
        self.spawn_entity(kind, pos, vec![])
    }
    fn remove_entity(&mut self, id: i32) -> Entity {
        self.actor_ids.remove(&id);
        self.ability_state.objects.remove(&id);
        let entity = self.entities.remove(&id).expect("indexed entity missing");
        self.orphan_satellites(&entity);
        assert!(!entity.reserved, "cannot remove a reserved container");
        self.bag_expiry.remove(&id);
        if let Some(camp) = entity.camp {
            tracing::debug!(
                camp,
                entity = id,
                kind = entity.kind,
                "family member removed"
            );
        }
        let cell = entity.pos.y as usize * self.map.width as usize + entity.pos.x as usize;
        assert!(
            self.cells[cell].remove(&id),
            "entity absent from spatial index"
        );
        entity
    }
    pub fn map_info(&self) -> Vec<u8> {
        let mut o = Writer::new();
        o.i32(self.map.width)
            .i32(self.map.height)
            .utf(&self.name)
            .u32(1)
            .i32(0)
            .u8(u8::from(self.allow_teleport()))
            .u8(1)
            .i16(0)
            .i16(0);
        o.0
    }
    #[cfg(test)]
    pub fn enter(&mut self, ch: Character, tx: Option<Output>) -> Result<i32> {
        // Combat and inventory fixtures need nearby, repeatable starting positions.
        let (x, y) = self.map.spawns[self.players.len() % self.map.spawns.len()];
        self.enter_at(
            ch,
            tx,
            Some(Pos { x, y }),
            AccountPresentation {
                guild: None,
                credits: 100,
                fame: 0,
                best_fame: BTreeMap::new(),
            },
        )
    }
    pub fn enter_at(
        &mut self,
        ch: Character,
        tx: Option<Output>,
        position: Option<Pos>,
        presentation: AccountPresentation,
    ) -> Result<i32> {
        if let Some(pos) = position {
            ensure!(
                pos.x.is_finite() && pos.y.is_finite() && self.walkable(pos),
                "prepared position is blocked or outside the world"
            );
        }
        ensure!(self.admission_open, "Realm is closed");
        ensure!(ch.alive && ch.hp > 0, "dead character");
        ch.effective(&self.content)?;
        ensure!(
            self.owner.is_none_or(|owner| owner == ch.account),
            "private world belongs to another account"
        );
        ensure!(
            self.guild
                .is_none_or(|(id, _)| presentation.guild.as_ref().is_some_and(|m| m.id == id)),
            "Guild Hall membership has been revoked."
        );
        ensure!(
            self.players.len() < self.max_players,
            "development world full"
        );
        ensure!(
            !self
                .players
                .values()
                .any(|p| p.character.account == ch.account),
            "account already in this world"
        );
        let id = allocate_id(&self.ids)?;
        let pos = if let Some(pos) = position {
            pos
        } else {
            let &(x, y) = self
                .map
                .spawns
                .choose(&mut self.rng)
                .context("world has no spawn cells")?;
            Pos { x, y }
        };
        let char_id = ch.id;
        if let Some(kind) = ch.pet_kind {
            self.attach_companion(id, kind, pos)?;
        }
        self.players.insert(
            id,
            Player {
                weapon_ticks: VecDeque::from([(-1, ch.items[0])]),
                character: ch,
                pos,
                departing: false,
                account_lists: AccountLists::default(),
                presentation,
                paused: false,
                native_update_acked: false,
                controller: if tx.is_none() {
                    Controller::Qa
                } else {
                    Controller::Human
                },
                tx,
                known: BTreeSet::new(),
                quest: None,
                statuses: BTreeMap::new(),
                native_motion_condition: 0,
                known_since: BTreeMap::new(),
                shoot_acks: VecDeque::new(),
                visible_projectiles: BTreeMap::new(),
                ground_hits: BTreeMap::new(),
                oxygen: if self.name == "Ocean Trench" { 100 } else { -1 },
                tiles: BTreeSet::new(),
                view_tiles: std::cell::RefCell::default(),
                death_started: false,
                killer: String::new(),
                movement_credit: 0.78,
                hp_fraction: 0.,
                mp_fraction: 0.,
                pending: false,
                experience_queued: 0,
                clears_queued: [0; crate::dungeon::DUNGEONS.len()],
                progress_started: false,
                death_pending: false,
                persistence_failure: None,
                bot_goal: None,
                bot_aim: None,
                next_bullet: 1,
                server_bullet: 128,
                seed: 1,
                last_attack: -1000,
                last_attack_at: 0,
                attack_credit: 1000.,
                attack_count: 0,
                attack_suppressed: None,
                last_move: 0,
                last_move_at: self.now,
                motion: ObservedMotion {
                    position: pos,
                    time: 0,
                    velocity: Pos { x: 0., y: 0. },
                    at: self.now,
                },
                goto_acks: VecDeque::new(),
                discarded_attack: None,
                tick: 0,
                last_motion: None,
                last_ability: None,
                boosts: Vec::new(),
                last_companion: None,
                ability_shot: None,
            },
        );
        let mut o = Writer::new();
        o.i32(id).i32(char_id);
        send(&self.players[&id], wire::CREATE_SUCCESS, o.0);
        self.updates(id, true)?;
        tracing::info!(
            player = id,
            character = char_id,
            world = %self.name,
            x = pos.x,
            y = pos.y,
            "player entered development world"
        );
        Ok(id)
    }
    #[cfg(test)]
    pub fn enter_autonomous(&mut self, ch: Character, tx: Output) -> Result<i32> {
        let id = self.enter(ch, Some(tx))?;
        self.players
            .get_mut(&id)
            .context("new controller missing")?
            .controller = Controller::Autonomous;
        Ok(id)
    }
    pub fn leave(&mut self, id: i32) -> Result<Character> {
        self.clear_abilities(id);
        self.cancel_trade(id, "Trade cancelled by departure.");
        let p = self.players.get(&id).context("player not found")?;
        ensure!(
            !p.pending && !p.death_pending,
            "inventory or death operation pending"
        );
        self.clear_companions(id);
        let p = self.players.remove(&id).unwrap();
        self.bullets.retain(|b| b.owner != id);
        tracing::info!(player = id, character = p.character.id, "player left");
        Ok(p.character)
    }
    pub fn stats(&self, id: i32, recipient: i32) -> Result<Vec<u8>> {
        let mut o = Writer::new();
        if let Some(p) = self.players.get(&id) {
            let values = p.effective(&self.content, self.now)?;
            o.i32(id)
                .f32(p.pos.x)
                .f32(p.pos.y)
                .i16(45 + 4 * i16::from(id == recipient));
            for (k, v) in [
                (0, values[0]),
                (1, p.character.hp),
                (2, 100),
                (3, values[1]),
                (4, p.character.mp),
                (5, crate::store::experience_goal(p.character.level)?),
                (
                    6,
                    p.character.exp - crate::store::level_start(p.character.level)?,
                ),
                (7, p.character.level),
            ] {
                stat(&mut o, k, v);
            }
            for (i, item) in p.character.items.iter().enumerate() {
                stat(&mut o, 8 + i as u8, *item);
            }
            for (k, v) in [
                (20, values[2]),
                (21, values[3]),
                (22, values[4]),
                (26, values[6]),
                (27, values[7]),
                (28, values[5]),
                (
                    29,
                    p.conditions(self.now) | if p.paused { 1 << 20 } else { 0 },
                ),
                (30, p.presentation.stars()),
            ] {
                stat(&mut o, k, v);
            }
            text_stat(&mut o, 31, &p.character.name);
            stat(&mut o, 32, p.character.tex1 as i32);
            stat(&mut o, 33, p.character.tex2 as i32);
            stat(&mut o, 38, p.character.account);
            if id == recipient {
                stat(&mut o, 35, p.presentation.credits);
                stat(&mut o, 39, p.presentation.fame);
                stat(&mut o, 64, p.oxygen);
                stat(&mut o, 65, i32::from(p.character.xp_boost.active));
            }
            for (i, boost) in [46, 47, 48, 49, 50, 53, 51, 52].into_iter().enumerate() {
                stat(&mut o, boost, values[i] - p.character.base[i]);
            }
            stat(
                &mut o,
                56,
                i32::from(crate::store::name_chosen(
                    p.character.account,
                    &p.character.name,
                )),
            );
            stat(&mut o, 57, p.character.fame);
            stat(
                &mut o,
                58,
                p.presentation
                    .next_quest(p.character.class, p.character.fame),
            );
            text_stat(
                &mut o,
                62,
                p.presentation
                    .guild
                    .as_ref()
                    .map_or("", |g| g.name.as_str()),
            );
            stat(
                &mut o,
                63,
                p.presentation.guild.as_ref().map_or(0, |g| g.rank),
            );
        } else {
            let e = self.entities.get(&id).context("entity missing")?;
            o.i32(id).f32(e.pos.x).f32(e.pos.y).i16(
                (4 + usize::from(!e.name.is_empty()) + e.items.len() + e.extra_stats.len()) as i16,
            );
            stat(&mut o, 0, self.content.get(e.kind)?.hp);
            stat(&mut o, 1, e.hp);
            stat(&mut o, 2, e.size);
            stat(&mut o, 29, e.conditions(self.now));
            if !e.name.is_empty() {
                text_stat(&mut o, 31, &e.name);
            }
            for (i, item) in e.items.iter().enumerate() {
                stat(&mut o, 8 + i as u8, *item);
            }
            for (id, value) in &e.extra_stats {
                stat(&mut o, *id, *value);
            }
        }
        Ok(o.0)
    }
    pub(crate) fn apply_condition(&mut self, id: i32, index: u8, duration_ms: u64) -> Result<()> {
        ensure!(
            (1..=28).contains(&index) && duration_ms > 0,
            "invalid condition application"
        );
        let deadline = self
            .now
            .checked_add(duration_ms)
            .context("condition deadline overflow")?;
        let timers = if let Some(p) = self.players.get_mut(&id) {
            if index == crate::condition::QUIET {
                p.character.mp = 0;
                p.mp_fraction = 0.;
            }
            &mut p.statuses
        } else {
            let e = self
                .entities
                .get_mut(&id)
                .context("condition target missing")?;
            if index == crate::condition::STASIS {
                if e.conditions(self.now) & crate::condition::bit(crate::condition::STASIS_IMMUNE)
                    != 0
                {
                    return Ok(());
                }
                e.statuses
                    .insert(crate::condition::STASIS_IMMUNE, (deadline, deadline + 3000));
            }
            if index == crate::condition::STUNNED
                && e.conditions(self.now) & crate::condition::bit(12) != 0
            {
                tracing::debug!(entity = id, index, "condition blocked by Stun Immunity");
                return Ok(());
            }
            &mut e.statuses
        };
        timers
            .entry(index)
            .and_modify(|old| *old = (self.now, old.1.max(deadline)))
            .or_insert((self.now, deadline));
        tracing::debug!(
            entity = id,
            condition = index,
            deadline,
            "timed condition applied"
        );
        Ok(())
    }
    fn defense(&self, id: i32) -> Result<i32> {
        let p = &self.players[&id];
        Ok(p.effective(&self.content, self.now)?[3])
    }
    fn quest_target(&self, id: i32) -> Option<i32> {
        if self.safe {
            return None;
        }
        let player = &self.players[&id];
        self.actors()
            .filter_map(|e| {
                let definition = &self.content.objects[&e.kind];
                if !definition.quest || e.hp <= 0 || !e.accessible(player.character.account) {
                    return None;
                }
                let (min, max) = match e.kind {
                    0x617 => (3, 8),
                    0x609 | 0x60d => (4, 9),
                    0x61f => (5, 10),
                    _ => (definition.quest_level?, 20),
                };
                (min <= player.character.level && player.character.level <= max).then_some((e, min))
            })
            .min_by(|(a, a_level), (b, b_level)| {
                b_level
                    .cmp(a_level)
                    .then_with(|| {
                        a.pos
                            .distance(player.pos)
                            .total_cmp(&b.pos.distance(player.pos))
                    })
                    .then_with(|| a.id.cmp(&b.id))
            })
            .map(|(e, _)| e.id)
    }
    fn broadcast_text(&self, name: &str, text: &str) -> usize {
        let packet = chat::text_packet(name, -1, 0, text);
        let mut recipients = 0;
        for p in self
            .players
            .values()
            .filter(|p| !p.departing && p.persistence_failure.is_none())
        {
            send(p, packet.id, packet.body.clone());
            recipients += 1;
        }
        recipients
    }
    fn visible(&self, id: i32) -> BTreeSet<i32> {
        let pos = self.players[&id].pos;
        self.entities_near(pos, 20.)
            .filter(|e| e.accessible(self.players[&id].character.account))
            .map(|e| e.id)
            .chain(self.quest_target(id))
            .chain(
                self.players
                    .iter()
                    .filter(|(_, p)| p.pos.distance(pos) < 20.)
                    .map(|(i, _)| *i),
            )
            .collect()
    }
    pub fn updates(&mut self, id: i32, initial: bool) -> Result<()> {
        let started = Instant::now();
        let mut visible = self.visible(id);
        // The native minimap tracks players across this world, including outside explored terrain.
        visible.extend(self.players.keys().copied());
        let previous = &self.players[&id].known;
        let mut added: Vec<i32> = visible.difference(previous).copied().collect();
        // Private-container owner stats dereference the native map's self player.
        added.sort_by_key(|entity| (*entity != id, *entity));
        let dropped: Vec<i32> = previous.difference(&visible).copied().collect();
        let pos = self.players[&id].pos;
        let mut tiles = Vec::new();
        for y in ((pos.y as i32 - 20).max(0))..=(pos.y as i32 + 20).min(self.map.height - 1) {
            for x in ((pos.x as i32 - 20).max(0))..=(pos.x as i32 + 20).min(self.map.width - 1) {
                let dx = x - pos.x as i32;
                let dy = y - pos.y as i32;
                if dx * dx + dy * dy <= 20 * 20
                    && !self.players[&id].tiles.contains(&(x as i16, y as i16))
                {
                    tiles.push((
                        x as i16,
                        y as i16,
                        self.map.cells[(y * self.map.width + x) as usize].ground,
                    ));
                }
            }
        }
        if initial || !tiles.is_empty() || !added.is_empty() || !dropped.is_empty() {
            let mut o = Writer::new();
            o.i16(i16::try_from(tiles.len())?);
            let p = self.players.get_mut(&id).unwrap();
            for (x, y, ground) in tiles {
                p.tiles.insert((x, y));
                p.character.tiles += 1;
                o.i16(x).i16(y).u8(ground);
            }
            o.i16(added.len() as i16);
            for entity in added {
                let kind = if self.players.contains_key(&entity) {
                    self.players[&entity].character.class
                } else {
                    self.entities[&entity].kind
                };
                o.i16(kind as i16);
                o.0.extend(self.stats(entity, id)?);
            }
            o.i16(dropped.len() as i16);
            for entity in dropped {
                o.i32(entity);
            }
            send(&self.players[&id], wire::UPDATE, o.0);
        }
        let p = self.players.get_mut(&id).unwrap();
        p.known_since.retain(|entity, _| visible.contains(entity));
        for entity in &visible {
            p.known_since.entry(*entity).or_insert(self.now);
        }
        p.known = visible;
        let quest = self.quest_target(id);
        let p = self.players.get_mut(&id).unwrap();
        if initial || p.quest != quest {
            p.quest = quest;
            let mut out = Writer::new();
            out.i32(quest.unwrap_or(-1));
            send(p, 34, out.0);
            tracing::debug!(player = id, quest, "native quest target updated");
        }
        self.performance
            .borrow_mut()
            .record("visibility", started.elapsed());
        Ok(())
    }
    #[cfg(test)]
    fn validate_move(&self, id: i32, pos: Pos, time: i32) -> Result<f32> {
        self.validate_move_at(id, pos, time, self.now)
    }
    fn validate_move_at(&self, id: i32, pos: Pos, time: i32, received: u64) -> Result<f32> {
        ensure!(
            pos.x.is_finite() && pos.y.is_finite(),
            "non-finite movement"
        );
        let p = self.players.get(&id).context("player missing")?;
        ensure!(
            p.active(),
            "dead, departing or quarantined player cannot move"
        );
        ensure!(time >= 0, "negative movement time");
        let elapsed = if p.last_move == 0 {
            100.
        } else {
            (i64::from(time) - i64::from(p.last_move)) as f32
        };
        ensure!(
            elapsed >= 0.,
            "movement time reversed: client_time={time} previous_client_time={}",
            p.last_move
        );
        let cell = self
            .map
            .cell(p.pos.x, p.pos.y)
            .context("player outside map")?;
        let speed = p.speed(&self.content, self.now)? * self.content.grounds[&cell.ground];
        let push = self.content.pushes[&cell.ground];
        let distance = p.pos.distance(pos);
        let input_distance = |delta| {
            p.pos.distance(Pos {
                x: pos.x - push.0 * delta,
                y: pos.y - push.1 * delta,
            })
        };
        ensure!(
            p.conditions(self.now) & PARALYZED == 0 || distance <= 0.001,
            "paralyzed movement"
        );
        ensure!(
            input_distance(elapsed) <= speed * elapsed + 0.3,
            "movement exceeds speed: from=({}, {}) to=({}, {}) distance={distance} allowed={} speed={speed} client_elapsed_ms={elapsed}",
            p.pos.x,
            p.pos.y,
            pos.x,
            pos.y,
            speed * elapsed + 0.3
        );
        let server_elapsed = received
            .checked_sub(p.last_move_at)
            .context("movement server clock regressed")?;
        let credit = (p.movement_credit + speed * server_elapsed as f32).min(speed * 350. + 0.3);
        let charged = input_distance((server_elapsed as f32).min(350.));
        tracing::debug!(
            player = id,
            client_time = time,
            received,
            ground = cell.ground,
            speed,
            client_elapsed_ms = elapsed,
            server_elapsed_ms = server_elapsed,
            previous_credit = p.movement_credit,
            credit,
            charged,
            from_x = p.pos.x,
            from_y = p.pos.y,
            to_x = pos.x,
            to_y = pos.y,
            "native motion budget"
        );
        ensure!(
            charged <= credit + 0.001,
            "movement exceeds server time budget: distance={distance} credit={credit} server_elapsed_ms={server_elapsed} client_elapsed_ms={elapsed} previous_credit={}",
            p.movement_credit
        );
        let samples = (distance / 0.1).ceil().max(1.) as usize;
        ensure!(
            (1..=samples).all(|i| {
                let f = i as f32 / samples as f32;
                self.player_step_valid(
                    p.pos,
                    Pos {
                        x: p.pos.x + (pos.x - p.pos.x) * f,
                        y: p.pos.y + (pos.y - p.pos.y) * f,
                    },
                )
            }),
            "movement across a blocked tile"
        );
        Ok(credit - charged)
    }
    #[cfg(test)]
    pub fn native_move_to(&mut self, id: i32, pos: Pos, time: i32) -> Result<()> {
        // Skipped simulation ticks must not reduce real movement funding or desynchronize SHOOT prediction.
        self.native_move_received(id, pos, (time, self.clock.now()))
            .map(|_| ())
    }
    pub fn native_move_received(
        &mut self,
        id: i32,
        pos: Pos,
        (time, received): (i32, Instant),
    ) -> Result<NativeInput> {
        let queued_ms = received.elapsed().as_millis();
        let received = received
            .checked_duration_since(self.attack_epoch)
            .context("native MOVE predates world epoch")?
            .as_millis() as u64;
        tracing::debug!(
            player = id,
            client_time = time,
            received,
            queued_ms,
            x = pos.x,
            y = pos.y,
            "native MOVE ingress"
        );
        if self.discard_correcting_move(id, pos, time)? {
            return Ok(NativeInput::DiscardedUntilGotoAck);
        }
        let sentinel = pos.x == -1. && pos.y == -1.;
        if !sentinel {
            self.validate_native_position(id, pos, time)?;
        }
        let p = self.players.get_mut(&id).context("native player missing")?;
        ensure!(time >= p.last_move, "native movement clock regressed");
        ensure!(received >= p.last_move_at, "native ingress clock regressed");
        let speed = p.speed(&self.content, self.now)? * 1000.;
        p.record_motion(if sentinel { p.pos } else { pos }, time, self.now, speed);
        if !sentinel {
            if p.pos.distance(pos) > 0.001 {
                p.last_motion = Some(((pos.y - p.pos.y).atan2(pos.x - p.pos.x), self.now));
            }
            p.pos = pos;
        }
        p.last_move = time;
        p.last_move_at = received;
        Ok(NativeInput::Applied)
    }
    pub fn move_to(&mut self, id: i32, pos: Pos, time: i32) -> Result<()> {
        self.move_timed(id, pos, time, self.now)
    }
    fn move_timed(&mut self, id: i32, pos: Pos, time: i32, received: u64) -> Result<()> {
        let p = self.players.get(&id).context("player missing")?;
        ensure!(
            p.last_move == 0 || i64::from(time) - i64::from(p.last_move) <= 1000,
            "movement time jump"
        );
        // MOVE precedes condition updates; accept either form across a toggle.
        let sentinel = pos.x == -1. && pos.y == -1.;
        let immobile = p.paused || p.conditions(self.now) & PARALYZED != 0;
        let correct = immobile && !sentinel && p.pos.distance(pos) > 0.001;
        let stationary = immobile || sentinel;
        ensure!(pos.x.is_finite() && pos.y.is_finite(), "invalid position");
        let pos = if stationary { p.pos } else { pos };
        let remaining_credit = self.validate_move_at(id, pos, time, received)?;
        let p = self.players.get_mut(&id).unwrap();
        if !stationary {
            p.movement_credit = remaining_credit;
        }
        let speed = p.speed(&self.content, self.now)? * 1000.;
        p.record_motion(pos, time, self.now, speed);
        if p.pos.distance(pos) > 0.001 {
            p.last_motion = Some(((pos.y - p.pos.y).atan2(pos.x - p.pos.x), self.now));
        }
        p.pos = pos;
        p.last_move = time;
        p.last_move_at = received;
        if correct {
            self.correct_position(id, time, "immobilized movement")?;
        }
        Ok(())
    }
    fn player_step_valid(&self, from: Pos, to: Pos) -> bool {
        let crosses =
            |a: f32, b: f32| (a % 0.5 == 0. && a != b) || (a * 2.).floor() != (b * 2.).floor();
        // The client only revalidates when departing a half-cell boundary or crossing one.
        (!crosses(from.x, to.x) && !crosses(from.y, to.y))
            || self.player_position_valid(Some(from), to)
    }
    fn player_position_valid(&self, from: Option<Pos>, pos: Pos) -> bool {
        if !pos.x.is_finite() || !pos.y.is_finite() {
            return false;
        }
        let Some(cell) = self.map.cell(pos.x, pos.y) else {
            return false;
        };
        let own = (pos.x.floor() as i32, pos.y.floor() as i32);
        let check_own = from.is_none_or(|p| (p.x.floor() as i32, p.y.floor() as i32) != own);
        if check_own && self.content.grounds[&cell.ground] <= 0. {
            return false;
        }
        let side = |n: f32| {
            if n.fract() < 0.5 {
                -1
            } else if n.fract() > 0.5 {
                1
            } else {
                0
            }
        };
        let (dx, dy) = (side(pos.x), side(pos.y));
        let neighbors = [
            (own.0 + dx, own.1),
            (own.0, own.1 + dy),
            (own.0 + dx, own.1 + dy),
        ];
        if neighbors.iter().any(|&(x, y)| {
            (x, y) != own
                && self
                    .map
                    .cell(x as f32, y as f32)
                    .is_none_or(|c| c.ground == 255)
        }) {
            return false;
        }
        self.near_entities(pos, pos).all(|e| {
            let cell = (e.pos.x.floor() as i32, e.pos.y.floor() as i32);
            if check_own && cell == own {
                !self.content.objects[&e.kind].blocks
            } else if cell != own && neighbors.contains(&cell) {
                !self.content.objects[&e.kind].full_occupy
            } else {
                true
            }
        })
    }
    pub fn walkable(&self, pos: Pos) -> bool {
        let Some(c) = self.map.cell(pos.x, pos.y) else {
            return false;
        };
        if self.content.grounds[&c.ground] <= 0. {
            return false;
        }
        self.entities_at(pos)
            .all(|e| !self.content.objects[&e.kind].blocks)
    }
    // Immutable map scenery participates in collision/visibility, not per-tick AI scans.
    fn actors(&self) -> impl Iterator<Item = &Entity> {
        self.actor_ids.iter().map(|id| {
            self.entities
                .get(id)
                .expect("simulated entity missing from storage")
        })
    }
    fn entities_near(&self, pos: Pos, radius: f32) -> impl Iterator<Item = &Entity> {
        self.near_entities(
            Pos {
                x: pos.x - radius,
                y: pos.y - radius,
            },
            Pos {
                x: pos.x + radius,
                y: pos.y + radius,
            },
        )
        .filter(move |e| e.pos.distance(pos) < radius)
    }
    fn entities_at(&self, pos: Pos) -> impl Iterator<Item = &Entity> {
        self.map.cell(pos.x, pos.y).into_iter().flat_map(move |_| {
            self.cells[pos.y as usize * self.map.width as usize + pos.x as usize]
                .iter()
                .map(|id| self.entities.get(id).expect("occupied cell missing entity"))
        })
    }
    fn near_entities(&self, a: Pos, b: Pos) -> impl Iterator<Item = &Entity> {
        let x0 = (a.x.min(b.x) as i32 - 1).max(0);
        let x1 = (a.x.max(b.x) as i32 + 1).min(self.map.width - 1);
        let y0 = (a.y.min(b.y) as i32 - 1).max(0);
        let y1 = (a.y.max(b.y) as i32 + 1).min(self.map.height - 1);
        (y0..=y1).flat_map(move |y| {
            (x0..=x1).flat_map(move |x| {
                self.cells[(y * self.map.width + x) as usize]
                    .iter()
                    .map(|id| self.entities.get(id).expect("occupied cell missing entity"))
            })
        })
    }

    pub fn native_shoot_received(
        &mut self,
        id: i32,
        (time, received): (i32, Instant),
        bullet: u8,
        kind: u16,
        start: Pos,
        angle: f32,
    ) -> Result<NativeInput> {
        let queued_ms = received.elapsed().as_millis();
        let received = received
            .checked_duration_since(self.attack_epoch)
            .context("native SHOOT predates world epoch")?
            .as_millis() as u64;
        tracing::debug!(
            player = id,
            client_time = time,
            received,
            queued_ms,
            bullet,
            kind,
            x = start.x,
            y = start.y,
            angle,
            "native SHOOT ingress"
        );
        let ability = self.players.get(&id).is_some_and(
            |p| matches!(p.ability_shot, Some((k, t, _, _)) if k == kind && t == time),
        );
        if !ability && self.discard_correcting_shot(id, time, bullet, kind, start, angle)? {
            return Ok(NativeInput::DiscardedUntilGotoAck);
        }
        let outcome = self.shoot_timed(id, (time, received, true), bullet, kind, start, angle)?;
        self.players.get_mut(&id).unwrap().discarded_attack = None;
        Ok(outcome)
    }
    // Fixture tests drive fixed ticks without sleeping, so script shots use simulation time.
    pub fn shoot(
        &mut self,
        id: i32,
        time: i32,
        bullet: u8,
        kind: u16,
        start: Pos,
        angle: f32,
    ) -> Result<()> {
        self.shoot_timed(id, (time, self.now, false), bullet, kind, start, angle)
            .map(|_| ())
    }
    fn shoot_timed(
        &mut self,
        id: i32,
        (time, received, native): (i32, u64, bool),
        bullet: u8,
        kind: u16,
        start: Pos,
        angle: f32,
    ) -> Result<NativeInput> {
        ensure!(angle.is_finite(), "invalid shot angle");
        ensure!(
            start.x.is_finite() && start.y.is_finite(),
            "non-finite shot origin"
        );
        let p = self.players.get(&id).context("player missing")?;
        ensure!(
            p.active(),
            "dead, departing or quarantined player cannot shoot"
        );

        let ability =
            native && matches!(p.ability_shot, Some((k, t, _, _)) if k == kind && t == time);
        let denied_ability = ability && matches!(p.ability_shot, Some((_, _, false, _)));
        ensure!(
            p.character.items[0] == kind as i32
                || native && p.advertised_weapon(kind)
                || ability && p.character.items[1] == kind as i32,
            "weapon does not match equipped item"
        );
        let item = self.content.get(kind)?;
        ensure!(item.shots > 0, "weapon projectile count is zero");
        let shot_index = if ability {
            p.ability_shot.unwrap().3
        } else if time == p.last_attack {
            p.attack_count
        } else {
            0
        };
        // The client displaces every muzzle along central aim, not each projectile's spread angle.
        let aim =
            angle + item.arc.to_radians() * ((item.shots - 1) as f32 / 2. - shot_index as f32);
        // Prediction cannot advance movement credit or authoritative position.
        let center = Pos {
            x: start.x - 0.3 * aim.cos(),
            y: start.y - 0.3 * aim.sin(),
        };
        let condition = p.conditions(self.now);
        let origin = if native {
            self.validate_native_position(id, center, time).map(|_| 0.)
        } else if !p.paused && p.pos.distance(center) > 0.001 {
            self.validate_move_at(id, center, time, received)
        } else {
            Ok(0.)
        };
        // Drop stale predicted shots for one MOVE time-jump window around movement changes.
        let displaced = ability && p.correcting()?
            || origin.is_err()
                && (condition & PARALYZED != 0
                    || p.statuses.iter().any(|(effect, (since, until))| {
                        crate::condition::bit(*effect) & (SLOWED | SPEEDY | PARALYZED) != 0
                            && (self.now - since < 1000
                                || self.now >= *until && self.now - until < 1000)
                    }));
        if !displaced {
            origin.with_context(|| RejectedNativeShot(format!("invalid shot origin: player=({}, {}) origin=({}, {}) angle={} shot_time={} move_time={} server_time={} last_move_at={}", p.pos.x, p.pos.y, start.x, start.y, angle, time, p.last_move, self.now, p.last_move_at)))?;
        }
        let spec = item
            .projectiles
            .get(&0)
            .cloned()
            .context("weapon has no projectile zero")?;
        // The selected client's integer attack period cannot fund a fractional charge indefinitely.
        let frequency =
            0.0015 + (0.008 - 0.0015) * p.effective(&self.content, self.now)?[5] as f64 / 75.;
        // Expired buffs remain for the existing one-second native transition window.
        let late_berserk = native
            && p.statuses
                .get(&crate::condition::BERSERK)
                .is_some_and(|(_, until)| self.now >= *until && self.now - until < 1000);
        let berserk =
            condition & crate::condition::bit(crate::condition::BERSERK) != 0 || late_berserk;
        let normal_cooldown =
            (1. / frequency / f64::from(item.rate) / if berserk { 1.5 } else { 1. }).floor() as f32;
        let effective_cooldown =
            reconciliation::attack_cooldown(p, &self.content, self.now, item.rate)?;
        let cooldown =
            if late_berserk && condition & crate::condition::bit(crate::condition::DAZED) == 0 {
                normal_cooldown
            } else {
                effective_cooldown
            };
        let admission = if native { normal_cooldown } else { cooldown };
        let p = self.players.get_mut(&id).unwrap();
        ensure!(time >= 0, "negative shot time");
        ensure!(shot_index < item.shots, "too many projectiles in attack");
        ensure!(bullet == p.next_bullet, "unexpected player bullet counter");
        if !ability && time != p.last_attack {
            let attack_elapsed = i64::from(time) - i64::from(p.last_attack);
            ensure!(attack_elapsed >= admission as i64 - 2, "attack too fast");
            let elapsed = received
                .checked_sub(p.last_attack_at)
                .context("attack receive clock regressed")?;
            // Delivery can batch correctly spaced native shots; their clock, not ingress spacing, funds cadence.
            let funding = if native {
                attack_elapsed as f32
            } else {
                elapsed as f32
            };
            let credit = (p.attack_credit + funding).min(cooldown * 2.);
            ensure!(
                native || credit >= admission - 50.,
                RejectedNativeShot(format!(
                    "attack exceeds server time budget: credit={credit} cooldown={cooldown} received_time={received} simulation_time={} last_received_time={} shot_time={time} last_shot_time={}",
                    self.now, p.last_attack_at, p.last_attack
                ))
            );
            p.attack_suppressed =
                if attack_elapsed < cooldown as i64 - 2 || (!native && credit < cooldown - 50.) {
                    Some(AttackSuppression::EffectiveCooldown)
                } else {
                    None
                };
            p.attack_credit = credit
                - match p.attack_suppressed {
                    Some(AttackSuppression::EffectiveCooldown) => normal_cooldown,
                    None => cooldown,
                };
            p.last_attack_at = received;
            p.last_attack = time;
            p.attack_count = 0;
        }
        p.next_bullet = (bullet + 1) % 128;
        if ability {
            if shot_index + 1 == item.shots {
                p.ability_shot = None;
            } else {
                p.ability_shot.as_mut().unwrap().3 += 1;
            }
        } else {
            p.attack_count += 1;
        }
        let draw = p.roll_damage(&spec);
        // In-flight native SHOOT packets must still consume their counter/RNG draw.
        if p.paused
            || displaced
            || denied_ability
            || !ability && (condition & STUNNED != 0 || p.attack_suppressed.is_some())
        {
            tracing::debug!(
                player = id,
                condition,
                displaced,
                denied_ability,
                suppressed = ?p.attack_suppressed,
                "condition suppressed shot"
            );
            return Ok(NativeInput::Applied);
        }
        p.character.shots += 1;
        let multiplier = if ability {
            1.
        } else if condition & crate::condition::bit(crate::condition::WEAK) != 0 {
            0.5
        } else {
            (0.5 + (f64::from(p.effective(&self.content, self.now)?[2]) / 75.) * (2. - 0.5))
                * if condition & crate::condition::bit(crate::condition::DAMAGING) != 0 {
                    1.5
                } else {
                    1.
                }
        };
        let damage = (f64::from(draw) * multiplier) as i32;
        self.bullets.push(Bullet {
            owner: id,
            source_kind: p.character.class,
            id: bullet,
            friendly: true,
            start,
            pos: start,
            angle,
            born: self.now,
            damage,
            spec,
            hit: BTreeSet::new(),
        });
        let mut o = Writer::new();
        o.u8(bullet).i32(id).i16(kind as i16).f32(angle);
        for (other, p) in &self.players {
            if *other != id && p.known.contains(&id) {
                send(p, 74, o.0.clone());
            }
        }
        tracing::debug!(player = id, bullet, damage, "player shot");
        Ok(NativeInput::Applied)
    }
    pub fn native_use_item(
        &mut self,
        id: i32,
        time: i32,
        slot: Slot,
        pos: Pos,
    ) -> Result<Option<InventoryPlan>> {
        let ability = slot.kind >= 0
            && slot.slot == 1
            && slot.object == id
            && self
                .players
                .get(&id)
                .is_some_and(|p| p.character.items[1] == slot.kind)
            && self.content.get(slot.kind as u16)?.activate == "Shoot";
        ensure!(!ability || time >= 0, "negative ability time");
        ensure!(
            !ability || (1..=5).contains(&self.content.get(slot.kind as u16)?.shots),
            "unsupported native ability projectile count"
        );
        let plan = self.use_item(id, slot, pos);
        if ability {
            // Even rejected local predictions consume a native bullet ID and damage RNG draw.
            self.players.get_mut(&id).unwrap().ability_shot =
                Some((slot.kind as u16, time, plan.is_ok(), 0));
        }
        plan
    }
    pub fn use_item(&mut self, id: i32, slot: Slot, pos: Pos) -> Result<Option<InventoryPlan>> {
        self.cancel_trade(id, "Trade cancelled by inventory mutation.");
        let p = self.players.get(&id).context("player missing")?;
        ensure!(!p.paused, "Cannot use items while paused.");
        ensure!(
            p.active() && !p.pending && p.character.hp > 0,
            "player is busy, departing, quarantined or dead"
        );
        let source = if slot.object == id {
            &p.character.items
        } else {
            let bag = self
                .entities
                .get(&slot.object)
                .context("item ownership mismatch")?;
            ensure!(
                !bag.reserved
                    && bag.accessible(p.character.account)
                    && p.pos.distance(bag.pos) < 1.5
                    && self.content.get(bag.kind)?.class == "Container",
                "item container inaccessible"
            );
            &bag.items
        };
        ensure!(
            source.get(slot.slot) == Some(&slot.kind),
            "item ownership mismatch"
        );
        ensure!(slot.kind >= 0, "empty item slot");
        let item = self.content.get(slot.kind as u16)?.clone();
        if item.xp_boost {
            ensure!(
                item.kind == 0xc42 && item.consumable && item.activations == 0,
                "unsupported XP booster"
            );
            ensure!(
                p.character.level < 20,
                "XP boost requires a character below level 20"
            );
            ensure!(!p.character.xp_boost.active, "XP boost is already active");
            ensure!(
                !p.progress_queued(),
                "character progress is awaiting commit"
            );
            let mut plan = self.consumable_plan(id, slot, -1);
            plan.xp_boost = Some(slot);
            return Ok(Some(plan));
        }
        if !item.abilities.is_empty() {
            ensure!(slot.object == id, "ability must be equipped");
            self.activate_ability(id, slot, &item, pos)?;
            return Ok(None);
        }
        if matches!(
            item.activate.as_str(),
            "Dye" | "Pet" | "PermaPet" | "Create" | "UnlockPortal"
        ) {
            ensure!(
                slot.object == id,
                "item activation requires owned inventory"
            );
        }
        match item.activate.as_str() {
            "Dye" => {
                ensure!(
                    item.class == "Dye"
                        && item.consumable
                        && item.activations == 1
                        && item.textures.iter().any(Option::is_some),
                    "invalid appearance consumable"
                );
                let (mut tex1, mut tex2) = (p.character.tex1, p.character.tex2);
                if let Some(value) = item.textures[0] {
                    tex1 = value;
                }
                if let Some(value) = item.textures[1] {
                    tex2 = value;
                }
                let mut plan = self.consumable_plan(id, slot, -1);
                plan.appearance = Some((slot, tex1, tex2));
                Ok(Some(plan))
            }
            "IncrementStat" => {
                ensure!(
                    matches!(
                        item.kind,
                        0xa1f | 0xa20 | 0xa21 | 0xa34 | 0xa35 | 0xa4c | 0xae9 | 0xaea
                    ) && item.consumable
                        && item.activations == 1,
                    "unsupported permanent stat consumable"
                );
                let (stat, amount) = item
                    .stat_increase
                    .context("permanent potion activation missing")?;
                let index = crate::content::STATS
                    .iter()
                    .position(|(id, _)| *id == stat)
                    .context("unsupported permanent potion stat")?;
                let remaining =
                    self.content.classes[&p.character.class].caps[index] - p.character.base[index];
                ensure!(
                    amount > 0 && remaining > 0,
                    "stat is already at its class cap"
                );
                let amount = amount.min(remaining);
                let snapshot = p.character.clone();
                let mut plan = self.consumable_plan(id, slot, -1);
                plan.stat_growth = Some((snapshot, slot, index, amount));
                Ok(Some(plan))
            }
            "Pet" | "PermaPet" => self.begin_companion(id, slot, &item).map(Some),
            "Create" => self.begin_create(id, slot, &item).map(Some),
            "UnlockPortal" => self.begin_unlock(id, slot, &item).map(Some),
            _ => self.begin_consumable(id, slot, &item).map(Some),
        }
    }
    fn consumable_plan(&mut self, id: i32, slot: Slot, next: i32) -> InventoryPlan {
        let mut plan = self.plan(id, self.players[&id].character.items.clone(), None);
        if slot.object == id {
            plan.items[slot.slot] = next;
        } else {
            let bag = self.entities.get_mut(&slot.object).unwrap();
            let mut items = bag.items.clone();
            items[slot.slot] = next;
            bag.reserved = true;
            if let Some((container, vault)) = &self.vault
                && *container == slot.object
            {
                let mut vault = vault.clone();
                vault.items = items.clone();
                plan.vault = Some(vault);
            }
            plan.bag = Some((slot.object, items));
            plan.loot_slot = Some((slot.object, slot.slot));
        }
        self.players.get_mut(&id).unwrap().pending = true;
        plan
    }
    fn plan(&self, id: i32, items: Vec<i32>, bag: Option<(i32, Vec<i32>)>) -> InventoryPlan {
        let p = &self.players[&id];
        InventoryPlan {
            player: id,
            items,
            revision: p.character.revision,
            account: p.character.account,
            character: p.character.id,
            bag,
            effects: Vec::new(),
            stat_growth: None,
            appearance: None,
            xp_boost: None,
            companion: None,
            vault: None,
            dropped: None,
            unlock: None,
            collected: None,
            purchase: None,
            loot_slot: None,
        }
    }
    pub fn drop_item(&mut self, id: i32, slot: Slot) -> Result<InventoryPlan> {
        self.cancel_trade(id, "Trade cancelled by inventory mutation.");
        let p = self.players.get(&id).context("drop owner missing")?;
        ensure!(
            !p.pending && !p.death_pending && !p.departing && p.character.alive,
            "player busy, departing or dead"
        );
        let pos = p.pos;
        let account = p.character.account;
        let mut items = p.character.items.clone();
        let mut source = if slot.object == id {
            None
        } else {
            let e = self
                .entities
                .get(&slot.object)
                .context("drop container missing")?;
            ensure!(
                !e.reserved
                    && e.accessible(account)
                    && pos.distance(e.pos) < 1.5
                    && self.content.get(e.kind)?.class == "Container",
                "drop container inaccessible"
            );
            Some((slot.object, e.items.clone()))
        };
        let inventory = match &mut source {
            Some((_, items)) => items,
            None => &mut items,
        };
        ensure!(
            slot.kind >= 0 && inventory.get(slot.slot) == Some(&slot.kind),
            "empty or stale drop slot"
        );
        inventory[slot.slot] = -1;
        let dropped = self.reserve_drop(pos, account, slot.kind, false)?;
        if let Some((source, _)) = &source {
            self.entities.get_mut(source).unwrap().reserved = true;
        }
        self.players.get_mut(&id).unwrap().pending = true;
        let mut plan = self.plan(id, items, source);
        if let (Some((container, vault)), Some((source, items))) = (&self.vault, &plan.bag)
            && container == source
        {
            let mut vault = vault.clone();
            vault.items = items.clone();
            plan.vault = Some(vault);
        }
        if slot.object != id {
            plan.loot_slot = Some((slot.object, slot.slot));
        }
        plan.dropped = Some(dropped);
        Ok(plan)
    }
    fn reserve_drop(&mut self, pos: Pos, account: i32, kind: i32, private: bool) -> Result<i32> {
        let soulbound = self.content.get(u16::try_from(kind)?)?.soulbound;
        let dropped = self.loot_bag(pos, (soulbound || private).then_some(account), vec![kind])?;
        let bag = self.entities.get_mut(&dropped).unwrap();
        bag.kind = if soulbound { 0x503 } else { 0x500 };
        bag.published = false;
        bag.reserved = true;
        Ok(dropped)
    }
    pub fn swap(&mut self, id: i32, from: Slot, to: Slot) -> Result<InventoryPlan> {
        self.cancel_trade(id, "Trade cancelled by inventory mutation.");
        let p = self.players.get(&id).context("player missing")?;
        ensure!(
            !p.pending && !p.death_pending && !p.departing && p.character.alive,
            "player busy, departing or dead"
        );
        ensure!(
            from.object == id
                || to.object == id
                || self
                    .vault
                    .as_ref()
                    .is_some_and(|(chest, _)| { from.object == *chest && to.object == *chest }),
            "swap must involve own inventory or the vault chest"
        );
        let mut items = p.character.items.clone();
        let bag_id = if from.object != id {
            Some(from.object)
        } else if to.object != id {
            Some(to.object)
        } else {
            None
        };
        let mut bag = if let Some(b) = bag_id {
            let e = self.entities.get(&b).context("container missing")?;
            ensure!(
                !e.reserved
                    && e.accessible(p.character.account)
                    && p.pos.distance(e.pos) < 1.5
                    && self.content.get(e.kind)?.class == "Container",
                "container inaccessible"
            );
            e.items.clone()
        } else {
            Vec::new()
        };
        let get = |s: &Slot| -> Result<i32> {
            let a = if s.object == id { &items } else { &bag };
            Ok(*a.get(s.slot).context("invalid inventory slot")?)
        };
        let a = get(&from)?;
        let b = get(&to)?;
        ensure!(a == from.kind && b == to.kind, "stale inventory offer");
        let mut replacement = None;
        for (s, k) in [(&from, b), (&to, a)] {
            if s.object != id && k >= 0 {
                let item = self.content.get(u16::try_from(k)?)?;
                let container = &self.entities[&s.object];
                let definition = self.content.get(container.kind)?;
                let accepts = if item.soulbound {
                    definition.put_soulbound
                        && container.extra_stats.contains(&(54, p.character.account))
                } else {
                    definition.put_normal
                };
                if !accepts {
                    ensure!(
                        a >= 0 && b >= 0 && self.bag_expiry.contains_key(&s.object),
                        "container rejects this normal or Soulbound item"
                    );
                    replacement = Some(k);
                }
            }
            if s.object == id && s.slot < 4 && k >= 0 {
                let obj = self.content.get(k as u16)?;
                ensure!(
                    !obj.unsupported_equipment,
                    "equipment effect not implemented"
                );
                ensure!(
                    obj.slot_type == self.content.get(p.character.class)?.slots[s.slot],
                    "incompatible equipment slot"
                );
            }
        }
        if from.object == id {
            items[from.slot] = b
        } else {
            bag[from.slot] = b
        }
        if to.object == id {
            items[to.slot] = a
        } else {
            bag[to.slot] = a
        }
        let dropped = if let Some(kind) = replacement {
            // A swap must not put Soulbound gear in public loot or violate the source bag's put flags.
            let source = if from.object == id { to } else { from };
            bag[source.slot] = -1;
            let private = self.entities[&source.object]
                .extra_stats
                .contains(&(54, p.character.account));
            Some(self.reserve_drop(p.pos, p.character.account, kind, private)?)
        } else {
            None
        };
        if let Some(b) = bag_id {
            self.entities.get_mut(&b).unwrap().reserved = true;
        }
        self.players.get_mut(&id).unwrap().pending = true;
        let mut plan = self.plan(id, items, bag_id.map(|b| (b, bag)));
        plan.dropped = dropped;
        if let Some(container) = bag_id {
            let source = if from.object == container { &from } else { &to };
            plan.loot_slot = Some((container, source.slot));
            if source.kind >= 0
                && self.entities[&container].drop_slots.contains(&source.slot)
                && self.players[&id].controller == Controller::Human
                && self.content.get(source.kind as u16)?.class == "Equipment"
            {
                plan.collected = Some(source.kind as u16);
            }
        }
        if let (Some((container, vault)), Some((bag, items))) = (&self.vault, &plan.bag)
            && container == bag
        {
            let mut updated = vault.clone();
            updated.items = items.clone();
            plan.vault = Some(updated);
        }
        Ok(plan)
    }
    pub fn committed<T>(&mut self, id: i32, result: Result<T>) -> Result<T> {
        match result {
            Ok(value) => Ok(value),
            Err(error) => {
                self.poison(id, format!("{error:#}"))?;
                Err(error)
            }
        }
    }
    pub fn poison(&mut self, id: i32, error: String) -> Result<()> {
        self.players
            .get_mut(&id)
            .context("quarantine owner missing")
            .map(|p| p.quarantine(id, error))
    }
    #[tracing::instrument(skip_all, fields(player=plan.player, account=plan.account, character=plan.character, expected_revision=plan.revision))]
    pub fn finish_inventory(&mut self, plan: InventoryPlan, result: Result<i32>) -> Result<()> {
        ensure!(
            plan.unlock.is_none(),
            "portal unlock requires its directory commit"
        );
        let player = plan.player;
        let source = (plan.character, plan.revision);
        let companion = plan.companion.clone();
        let result = self.publish_inventory(plan, result);
        if let Err(error) = &result {
            self.discard_consumable(source);
            if let Err(cleanup) = self.discard_companion(&companion) {
                tracing::error!(player,%cleanup,"companion cleanup failed after rejected publication");
            }
            self.poison(player, format!("{error:#}"))?;
        }
        result
    }
    fn publish_inventory(&mut self, plan: InventoryPlan, result: Result<i32>) -> Result<()> {
        let revision = result?;
        let p = self
            .players
            .get_mut(&plan.player)
            .context("player disappeared during save")?;
        ensure!(
            p.pending
                && p.persistence_failure.is_none()
                && p.character.id == plan.character
                && p.character.account == plan.account
                && p.character.revision == plan.revision
                && plan.revision.checked_add(1) == Some(revision),
            "inventory completion owner, reservation or revision changed"
        );
        p.pending = false;
        if let Some((id, _)) = &plan.bag {
            ensure!(
                self.entities
                    .get(id)
                    .context("reserved bag disappeared")?
                    .reserved,
                "inventory source reservation lost"
            );
        }
        if let Some(dropped) = plan.dropped {
            ensure!(
                self.entities
                    .get(&dropped)
                    .context("reserved ground drop missing")?
                    .reserved,
                "ground drop reservation lost"
            );
        }
        if let Some((bag, slot)) = plan.loot_slot {
            self.entities
                .get_mut(&bag)
                .context("reserved drop source disappeared")?
                .drop_slots
                .remove(&slot);
        }
        let mut o = Writer::new();
        p.character.items = plan.items;
        p.character.revision = revision;
        if let Some((snapshot, _, index, amount)) = plan.stat_growth {
            ensure!(
                p.character.base == snapshot.base,
                "reserved potion base stats changed"
            );
            let cap = self.content.classes[&p.character.class].caps[index];
            ensure!(
                snapshot.base[index] + amount <= cap,
                "reserved potion exceeds current class cap"
            );
            p.character.base[index] = snapshot.base[index] + amount;
            tracing::info!(
                player = plan.player,
                stat = crate::content::STATS[index].0,
                base = p.character.base[index],
                revision,
                "permanent stat potion committed"
            );
        }
        if let Some((_, tex1, tex2)) = plan.appearance {
            p.character.tex1 = tex1;
            p.character.tex2 = tex2;
            tracing::info!(
                player = plan.player,
                tex1,
                tex2,
                revision,
                "appearance consumable committed"
            );
        }
        if plan.xp_boost.is_some() {
            p.character.xp_boost.active = true;
            tracing::info!(player = plan.player, revision, "XP boost committed");
        }
        let values = p.effective(&self.content, self.now)?;
        p.character.hp = p.character.hp.min(values[0]);
        p.character.mp = p.character.mp.min(values[1]);
        if self.prepared_seed.is_some()
            && let Some((bag, slot)) = plan.loot_slot
        {
            let item = self
                .entities
                .get(&bag)
                .and_then(|bag| bag.items.get(slot))
                .context("committed loot identity missing")?;
            tracing::info!(player=plan.player,character=p.character.id,bag,slot,item=*item,world=%self.name,"loot slot committed");
        }
        if let Some(mut vault) = plan.vault {
            vault.revision += 1;
            self.vault.as_mut().context("committed chest missing")?.1 = vault;
        }
        self.finish_consumable(plan.player, (plan.character, plan.revision), plan.effects)?;
        self.finish_companion(plan.player, plan.companion)?;
        if let Some((id, items)) = plan.bag {
            let bag = self.entities.get_mut(&id).unwrap();
            bag.items = items;
            bag.reserved = false;
        }
        if let Some(dropped) = plan.dropped {
            let e = self.entities.get_mut(&dropped).unwrap();
            e.reserved = false;
            e.published = true;
            self.bag_expiry.insert(dropped, self.now + 60_000);
        }
        tracing::info!(player = plan.player, revision, "inventory committed");
        let p = &self.players[&plan.player];
        if plan.purchase.is_some() {
            let packet = shop::reply(true, "Item purchased!");
            send(p, packet.id, packet.body);
        } else {
            o.i32(0);
            send(p, 4, o.0);
        }
        Ok(())
    }
    pub fn action(&mut self, id: i32, a: Action) -> Result<Option<InventoryPlan>> {
        let p = self.players.get(&id).context("player missing")?;
        ensure!(
            p.controller == Controller::Qa,
            "script actions require a QA-owned player"
        );
        self.controlled_action(id, a)
    }
    pub fn autonomous_action(&mut self, id: i32, a: Action) -> Result<Option<InventoryPlan>> {
        ensure!(
            self.players.get(&id).context("player missing")?.controller == Controller::Autonomous,
            "autonomous actions require an autonomous controller"
        );
        self.controlled_action(id, a)
    }
    fn controlled_action(&mut self, id: i32, a: Action) -> Result<Option<InventoryPlan>> {
        let p = self.players.get(&id).context("player missing")?;
        ensure!(
            p.active(),
            "dead, departing or quarantined player cannot act"
        );
        match a {
            Action::Move { x, y } => {
                ensure!(
                    self.player_position_valid(Some(p.pos), Pos { x, y }),
                    "goal outside map or blocked"
                );
                self.players
                    .get_mut(&id)
                    .context("player missing")?
                    .bot_goal = Some(Pos { x, y });
                Ok(None)
            }
            Action::Shoot { angle } => {
                ensure!(angle.is_finite(), "invalid aim");
                self.players.get_mut(&id).context("player missing")?.bot_aim = Some(angle);
                Ok(None)
            }
            Action::Stop => {
                let p = self.players.get_mut(&id).context("player missing")?;
                p.bot_goal = None;
                p.bot_aim = None;
                Ok(None)
            }
            Action::Use { slot, x, y } => {
                let kind = *self
                    .players
                    .get(&id)
                    .context("player missing")?
                    .character
                    .items
                    .get(slot)
                    .context("invalid slot")?;
                self.use_item(
                    id,
                    Slot {
                        object: id,
                        slot,
                        kind,
                    },
                    Pos { x, y },
                )
            }
            Action::Swap { from, to } => self.swap(id, from, to).map(Some),
            Action::Drop { slot } => self.drop_item(id, slot).map(Some),
        }
    }
    pub fn correct_position(&mut self, id: i32, time: i32, reason: &str) -> Result<()> {
        let p = self.players.get_mut(&id).context("player missing")?;
        let previous_time = p.last_move;
        ensure!(!p.correcting()?, "position correction is already pending");
        let mut out = Writer::new();
        out.i32(id).f32(p.pos.x).f32(p.pos.y);
        reconciliation::goto(p, true, out.0);
        tracing::warn!(
            player = id,
            previous_time,
            client_time = time,
            reason,
            "movement rejected; sent authoritative GOTO"
        );
        Ok(())
    }
    pub fn pending_progress(&mut self) -> Vec<(i32, Character)> {
        let mut plans = Vec::new();
        for (id, p) in &mut self.players {
            if !p.progress_queued() || p.pending || p.death_started {
                continue;
            }
            let mut snapshot = p.character.clone();
            let result = (|| -> Result<()> {
                snapshot.gain_experience(&self.content, p.experience_queued, &mut self.rng)?;
                for (count, queued) in std::iter::once(&mut snapshot.pirate_caves)
                    .chain(snapshot.dungeons.iter_mut())
                    .chain(std::iter::once(&mut snapshot.abysses))
                    .chain(std::iter::once(&mut snapshot.trenches))
                    .chain(std::iter::once(&mut snapshot.spider_dens))
                    .zip(p.clears_queued)
                {
                    *count = count
                        .checked_add(queued)
                        .context("dungeon clear counter overflow")?;
                }
                Ok(())
            })();
            if let Err(error) = result {
                p.quarantine(*id, format!("progress preparation failed: {error:#}"));
                continue;
            }
            snapshot.alive = true;
            p.experience_queued = 0;
            p.clears_queued = [0; crate::dungeon::DUNGEONS.len()];
            p.pending = true;
            p.progress_started = true;
            plans.push((*id, snapshot));
        }
        plans
    }
    pub fn finish_progress(&mut self, id: i32, snapshot: Character) -> Result<()> {
        let p = self
            .players
            .get_mut(&id)
            .context("progress owner missing")?;
        ensure!(
            p.pending
                && p.progress_started
                && p.character.id == snapshot.id
                && p.character.revision == snapshot.revision,
            "progress reservation changed"
        );
        let cleared_dungeon = snapshot.pirate_caves > p.character.pirate_caves
            || snapshot.dungeons != p.character.dungeons
            || snapshot.abysses > p.character.abysses
            || snapshot.trenches > p.character.trenches
            || snapshot.spider_dens > p.character.spider_dens;
        let leveled = snapshot.level > p.character.level;
        p.character.exp = snapshot.exp;
        p.character.xp_boost = snapshot.xp_boost;
        p.character.fame = snapshot.fame;
        p.character.level = snapshot.level;
        p.character.base = snapshot.base;
        p.character.pirate_caves = snapshot.pirate_caves;
        p.character.dungeons = snapshot.dungeons;
        p.character.abysses = snapshot.abysses;
        p.character.trenches = snapshot.trenches;
        p.character.spider_dens = snapshot.spider_dens;
        if leveled && !p.death_pending {
            let values = p.effective(&self.content, self.now)?;
            p.character.hp = values[0];
            p.character.mp = values[1];
            p.hp_fraction = 0.;
            p.mp_fraction = 0.;
        }
        p.pending = false;
        p.progress_started = false;
        tracing::info!(
            player = id,
            level = p.character.level,
            experience = p.character.exp,
            bonus_experience = p.character.xp_boost.bonus_exp,
            xp_boost = p.character.xp_boost.active,
            "progress committed"
        );
        if cleared_dungeon && let Some(exit) = self.clear_exit.take() {
            self.entities
                .get_mut(&exit)
                .context("reserved clear exit missing")?
                .published = true;
            tracing::info!(
                entity = exit,
                dungeon = self.name,
                "durable dungeon clear published exit"
            );
        }
        Ok(())
    }
    pub fn pending_deaths(&mut self) -> Vec<(i32, Character, String)> {
        let dying: Vec<_> = self
            .players
            .iter()
            .filter(|(_, p)| p.death_pending)
            .map(|(id, _)| *id)
            .collect();
        dying
            .into_iter()
            .filter_map(|id| {
                self.cancel_trade(id, "Trade cancelled by permanent death.");
                let p = self.players.get_mut(&id).unwrap();
                if p.pending || p.progress_queued() || p.death_started {
                    return None;
                }
                p.death_started = true;
                let mut c = p.character.clone();
                c.alive = false;
                Some((id, c, p.killer.clone()))
            })
            .collect()
    }
    pub fn finish_death(&mut self, id: i32) -> Result<()> {
        let p = self.players.get(&id).context("dying player missing")?;
        ensure!(
            p.death_started && p.death_pending && p.persistence_failure.is_none(),
            "death not pending or persistence failed"
        );
        let maxed = p
            .character
            .base
            .iter()
            .zip(self.content.classes[&p.character.class].caps)
            .filter(|(value, cap)| **value >= *cap)
            .count();
        let (kind, lifetime) = match maxed {
            0 if p.character.level == 1 => (0x723, Some(30_000)),
            0 if p.character.level < 20 => (0x724, Some(60_000)),
            0 => (0x725, Some(300_000)),
            7 => (0x734, None),
            8 => (0x735, None),
            n => (0x725 + n as u16, None),
        };
        let (pos, name) = (p.pos, p.character.name.clone());
        // Gates or terrain can become blocked after the dying player entered the tile.
        let grave = self.insert_entity(kind, pos, vec![])?;
        let entity = self.entities.get_mut(&grave).unwrap();
        entity.name = name;
        entity.expires = lifetime.map(|ms| self.now + ms);
        tracing::info!(player=id, grave, kind, maxed, expires=?entity.expires, "gravestone created");
        let p = self.players.get_mut(&id).unwrap();
        p.character.alive = false;
        p.character.xp_boost.active = false;
        p.character.items.fill(-1);
        p.death_pending = false;
        let mut out = Writer::new();
        out.i32(p.character.account)
            .i32(p.character.id)
            .utf(&p.killer);
        send(p, 41, out.0);
        tracing::info!(player=id,character=p.character.id,killed_by=%p.killer,"permanent death committed");
        self.clear_abilities(id);
        self.clear_companions(id);
        Ok(())
    }
    pub fn begin_departure(&mut self, id: i32) -> Result<Option<Character>> {
        self.cancel_trade(id, "Trade cancelled by departure.");
        let p = self
            .players
            .get_mut(&id)
            .context("departing player missing")?;
        ensure!(
            p.persistence_failure.is_none(),
            "quarantined persistence failure: {:?}",
            p.persistence_failure
        );
        p.departing = true;
        p.tx = None;
        p.bot_goal = None;
        p.bot_aim = None;
        if p.pending || p.death_pending || p.progress_queued() {
            return Ok(None);
        };
        p.pending = true;
        Ok(Some(p.character.clone()))
    }
    pub fn finish_departure(&mut self, id: i32) -> Result<()> {
        self.players
            .get_mut(&id)
            .context("departing player missing")?
            .pending = false;
        self.leave(id)?;
        Ok(())
    }
    pub fn inspect_map(&self, player: Option<i32>) -> Result<serde_json::Value> {
        let account = match player {
            Some(id) => {
                self.players
                    .get(&id)
                    .context("map observer missing")?
                    .character
                    .account
            }
            None => -1,
        };
        let blocked: Vec<_> = (0..self.map.height)
            .flat_map(|y| (0..self.map.width).map(move |x| (x, y)))
            .filter(|(x, y)| {
                !self.walkable(Pos {
                    x: *x as f32 + 0.5,
                    y: *y as f32 + 0.5,
                })
            })
            .collect();
        let entities: Vec<_> = self
            .entities
            .values()
            .filter(|e| e.accessible(account))
            .collect();
        Ok(
            serde_json::json!({"width":self.map.width,"height":self.map.height,"blocked":blocked,"entities":entities}),
        )
    }
    pub fn observe(&self, id: i32) -> Result<serde_json::Value> {
        let p = self.players.get(&id).context("player missing")?;
        let entities: Vec<_> = self
            .entities_near(p.pos, 20.)
            .filter(|e| e.accessible(p.character.account))
            .collect();
        let others: Vec<_> = self
            .players
            .iter()
            .filter(|(i, q)| **i != id && p.pos.distance(q.pos) < 20.)
            .map(|(i, q)| serde_json::json!({"id":i,"pos":q.pos,"name":q.character.name}))
            .collect();
        Ok(
            serde_json::json!({"tick":self.tick,"time_ms":self.now,"player":id,"pos":p.pos,"character":p.character,"entities":entities,"players":others,"bullets":self.bullets.len(),"own_projectiles":self.bullets.iter().filter(|b| b.friendly && b.owner == id).map(|b| serde_json::json!({"id":b.id,"start":b.start,"pos":b.pos,"angle":b.angle,"born":b.born})).collect::<Vec<_>>(),"pending":p.pending}),
        )
    }
    fn loot_bag(&mut self, pos: Pos, owner: Option<i32>, mut items: Vec<i32>) -> Result<i32> {
        ensure!(!items.is_empty(), "loot bag requires at least one item");
        ensure!(
            self.bag_expiry.len() < 128,
            "transient loot bag limit reached"
        );
        let mut appearance = 0;
        for item in &items {
            let object = self.content.get(*item as u16)?;
            appearance = appearance.max(object.bag_type);
            ensure!(
                owner.is_some() || !object.soulbound,
                "public loot cannot contain soulbound equipment"
            );
        }
        let kind = match appearance {
            0 => 0x500,
            1 => 0x503,
            2 => 0x507,
            3 => 0x508,
            4 => 0x509,
            _ => bail!("unsupported loot bag appearance"),
        };
        ensure!(
            items.len() <= 8,
            "loot exceeds bag capacity; split rewards before packaging"
        );
        items.resize(8, -1);
        let id = self.spawn_entity(kind, pos, items)?;
        let bag = self.entities.get_mut(&id).unwrap();
        bag.size = 80;
        if let Some(owner) = owner {
            bag.extra_stats.push((54, owner));
        }
        self.bag_expiry.insert(id, self.now + 60_000);
        tracing::info!(entity = id, owner, kind, "loot bag allocated");
        Ok(id)
    }
    fn loot_bags(&mut self, pos: Pos, owner: Option<i32>, items: &[i32]) -> Result<()> {
        for chunk in items.chunks(8) {
            let bag = self.loot_bag(pos, owner, chunk.to_vec())?;
            self.entities.get_mut(&bag).unwrap().drop_slots = (0..chunk.len()).collect();
        }
        Ok(())
    }
    fn defeat_entity(&mut self, id: i32) -> Result<()> {
        let e = self.remove_entity(id);
        let definition = &self.content.objects[&e.kind];
        // Reference Reproduce suppresses follower XP; Pentaract Spawn explicitly allows eye XP.
        let reward = if e.script.as_ref().is_some_and(|script| script.no_experience)
            || (e.quest.is_some() && matches!(e.kind, 0xd57 | 0xd58 | 0xd5a..=0xd5c))
        {
            0
        } else {
            (definition.hp as f32 * definition.xp_multiplier / 10.).ceil() as i32
        };
        for (id, p) in &mut self.players {
            if p.character.alive && !p.death_pending && !p.departing && p.pos.distance(e.pos) < 25.
            {
                let cap = (crate::store::experience_goal(p.character.level)
                    .expect("validated player level") as f32
                    * if definition.quest { 0.5 } else { 0.1 }) as i32;
                let amount = reward.min(cap);
                p.experience_queued = p
                    .experience_queued
                    .checked_add(amount)
                    .expect("experience queue overflow");
                if amount > 0 {
                    tracing::debug!(player = id, enemy = e.kind, amount, "experience queued");
                }
            }
        }
        if self.name == "Realm"
            && let Some((portal, chance)) = crate::dungeon::portal_drop(e.kind)
            && self.rng.gen_bool(chance)
        {
            self.portal_requests
                .push((portal, self.ocean_reward_position(&e)?));
            tracing::info!(
                enemy = e.kind,
                entity = id,
                portal,
                "defeat earned a dungeon portal"
            );
        }
        if let Err(error) = self.encounter_death(&e) {
            tracing::error!(enemy=e.kind, %error, "defeat mechanic failed");
        }
        if self.encounter
            && let Err(error) = self.drop_loot(&e)
        {
            tracing::error!(enemy=e.kind,%error,"earned encounter loot creation failed");
        }
        Ok(())
    }
    pub(super) fn finish_timed_defeat(&mut self, id: i32) -> Result<()> {
        let e = &self.entities[&id];
        let kind = e.kind;
        let owner = self
            .players
            .iter()
            .filter(|(_, p)| p.character.alive && !p.death_pending && !p.departing)
            .filter_map(|(id, p)| {
                e.damage
                    .get(&p.character.id)
                    .filter(|n| **n > 0)
                    .map(|n| (*id, *n))
            })
            .max_by_key(|(_, n)| *n)
            .map(|(id, _)| id);
        // Self-destruction has no final player bullet, so the largest eligible contributor owns its kill statistic.
        if let Some(owner) = owner {
            let p = self.players.get_mut(&owner).unwrap();
            p.character.kills += 1;
            if self.content.objects[&kind].quest {
                p.character.quests += 1;
            }
        }
        self.defeat_entity(id)?;
        tracing::info!(entity = id, kind, ?owner, "timed enemy defeat");
        Ok(())
    }
    fn drop_loot(&mut self, defeated: &Entity) -> Result<()> {
        if crate::endgame::HEROES.contains(&defeated.kind) {
            self.critical_deaths.push(defeated.id);
        } else if crate::quest::final_root(defeated.kind)
            && let Some(original) = defeated.quest
        {
            self.critical_deaths.push(original);
        }
        if let Some(index) = crate::dungeon::DUNGEONS
            .iter()
            .position(|(_, _, name, boss)| *name == self.name && *boss == defeated.kind)
        {
            ensure!(!self.cleared, "dungeon clear already recorded");
            self.cleared = true;
            for (id, p) in self.players.iter_mut().filter(|(_, p)| {
                p.character.alive
                    && !p.death_pending
                    && !p.departing
                    && p.pos.distance(defeated.pos) < 25.
                    && (index == 0 || defeated.damage.get(&p.character.id).is_some_and(|n| *n > 0))
            }) {
                p.clears_queued[index] = p.clears_queued[index]
                    .checked_add(1)
                    .context("queued dungeon clear overflow")?;
                tracing::info!(
                    player = id,
                    boss = defeated.id,
                    "dungeon participation clear queued"
                );
            }
            if self.clear_exit.is_none() {
                let kind = if defeated.kind == crate::dungeon::abyss::MALPHAS {
                    0x71c
                } else {
                    0x71d
                };
                let exit = self.spawn_exit(kind, defeated.pos)?;
                self.entities.get_mut(&exit).unwrap().published = false;
                self.clear_exit = Some(exit);
            }
            tracing::info!(
                boss = defeated.id,
                dungeon = self.name,
                "dungeon clear and exit staged for durable commit"
            );
        }
        if defeated.kind == crate::dungeon::spider::BOSS {
            return self.spider_loot(defeated);
        }
        let private = crate::loot::private(defeated.kind);
        // A flat 100-damage officer threshold exceeds the selected officers' entire HP.
        let threshold = if matches!(defeated.kind, 0xd56 | 0xd59) {
            self.content.objects[&defeated.kind].hp / 20
        } else if matches!(
            defeated.kind,
            0xd5a | 0xd60 | crate::dungeon::trench::HERMIT | 0x1705
        ) {
            self.content.objects[&defeated.kind].hp / 100
        } else if defeated.kind == 0x927 {
            100
        } else {
            (self.content.objects[&defeated.kind].hp / 10).max(1)
        };
        let mut eligible: Vec<_> = self
            .players
            .values()
            .filter(|p| p.character.alive && !p.death_pending && !p.departing)
            .filter_map(|p| {
                defeated
                    .damage
                    .get(&p.character.id)
                    .filter(|damage| **damage > if private { threshold } else { 0 })
                    .map(|damage| (p.character.account, *damage))
            })
            .collect();
        eligible.sort_by_key(|(_, damage)| std::cmp::Reverse(*damage));
        if eligible.is_empty() {
            return Ok(());
        }
        let mut drops = BTreeMap::<i32, Vec<i32>>::new();
        // Private boss rewards belong to every eligible character, not only the first eight.
        let attempts = if private {
            eligible.len()
        } else {
            (eligible.len() * 2).min(8)
        };
        for i in 0..attempts {
            let mut rewards = crate::loot::rewards(&self.content, defeated.kind, &mut self.rng);
            if defeated.kind == crate::dungeon::trench::THESSAL
                && i64::from(eligible[i % eligible.len()].1) * 100
                    >= i64::from(self.content.objects[&defeated.kind].hp) * 32
            {
                rewards.push(0xaea);
            }
            for item in rewards {
                let owner = if private {
                    eligible[i % eligible.len()].0
                } else {
                    -1
                };
                let items = drops.entry(owner).or_default();
                if !items.contains(&(item as i32)) || matches!(item, 0xa22 | 0xa23) {
                    items.push(item as i32);
                }
            }
        }
        for (owner, items) in drops {
            self.loot_bags(
                self.ocean_reward_position(defeated)?,
                (owner >= 0).then_some(owner),
                &items,
            )?;
        }
        Ok(())
    }
    fn populate_shore(&mut self) -> Result<()> {
        self.populate_shore_population()
    }
    fn repopulate(&mut self) -> Result<()> {
        for s in &mut self.population {
            if s.entity.is_some_and(|id| !self.entities.contains_key(&id)) {
                s.entity = None;
                s.ready = self.now + 60_000;
            }
        }
        for i in 0..self.population.len() {
            let s = &self.population[i];
            if s.camp.is_none()
                && s.entity.is_none()
                && self.now >= s.ready
                && self.population_space(self.content.objects[&s.kind].terrain, 1)?
                && self.players.values().all(|p| p.pos.distance(s.pos) >= 12.)
            {
                let id = self.spawn_entity(s.kind, s.pos, vec![])?;
                self.population[i].entity = Some(id);
                tracing::debug!(
                    entity = id,
                    kind = self.population[i].kind,
                    "ordinary population member replenished"
                );
            }
        }
        self.repopulate_families()
    }
    fn move_entity(&mut self, id: i32, goal: Pos, distance: f32) -> Result<()> {
        let old = self.entities[&id].pos;
        let length = old.distance(goal);
        if length <= 0.001 {
            self.entities.get_mut(&id).unwrap().last_step = Pos { x: 0., y: 0. };
            return Ok(());
        }
        let distance = distance.min(length);
        let pos = Pos {
            x: old.x + (goal.x - old.x) / length * distance,
            y: old.y + (goal.y - old.y) / length * distance,
        };
        let samples = (distance / 0.1).ceil().max(1.) as usize;
        ensure!(
            (1..=samples).all(|i| self.ocean_walkable(
                self.entities[&id].kind,
                Pos {
                    x: old.x + (pos.x - old.x) * i as f32 / samples as f32,
                    y: old.y + (pos.y - old.y) * i as f32 / samples as f32
                }
            )),
            "NPC movement crosses a blocked tile"
        );
        self.position_entity(id, pos)
    }
    fn position_entity(&mut self, id: i32, pos: Pos) -> Result<()> {
        ensure!(
            pos.x.is_finite() && pos.y.is_finite() && self.map.cell(pos.x, pos.y).is_some(),
            "entity position outside world"
        );
        let old = self.entities[&id].pos;
        let cell = |p: Pos| p.y as usize * self.map.width as usize + p.x as usize;
        assert!(
            self.cells[cell(old)].remove(&id),
            "moving NPC missing from spatial index"
        );
        self.cells[cell(pos)].insert(id);
        let e = self.entities.get_mut(&id).unwrap();
        e.pos = pos;
        e.last_step = Pos {
            x: pos.x - old.x,
            y: pos.y - old.y,
        };
        Ok(())
    }
    fn npc_step(&mut self) -> Vec<(i32, Vec<u8>)> {
        use crate::encounter::{Motion, profile};
        let mut packets = Vec::new();
        if !self.encounter || self.safe {
            return packets;
        }
        let delta = self.now.saturating_sub(self.last_npc_step);
        self.last_npc_step = self.now;
        let ids: Vec<_> = self
            .actors()
            .filter(|e| {
                let object = &self.content.objects[&e.kind];
                // Breakable scenery takes damage but has no movement/attack state machine.
                object.class != "Wall"
                    && !matches!(e.kind, 0xd5f | 0xd60)
                    && (object.enemy || matches!(e.kind, 0x173 | 0xd7d))
            })
            .map(|e| e.id)
            .collect();
        for id in ids {
            let Some(entity) = self.entities.get(&id) else {
                continue;
            };
            let kind = entity.kind;
            let pos = entity.pos;
            if entity.hp == self.content.objects[&kind].hp
                && (kind == crate::dungeon::abyss::IDOL || kind == 0xd7e && entity.stage.is_none())
            {
                continue;
            }
            let profile = profile(kind).expect("validated encounter profile");
            // Self-destruction is a deadline, not target acquisition or a weapon cooldown.
            let shield_burst = self.shield_burst(id);
            let burst = shield_burst
                || (kind == 0x91a
                    && entity.expires == Some(self.now + 50)
                    && self.now >= entity.next_shot);
            if !burst
                && entity.conditions(self.now) & crate::condition::bit(crate::condition::STASIS)
                    != 0
            {
                self.suspend_npc(id, delta);
                continue;
            }
            let nearest = self
                .players
                .values()
                .filter(|p| {
                    p.character.alive
                        && !p.death_pending
                        && !p.departing
                        && !p.paused
                        && (kind == 0x6d8
                            || p.conditions(self.now)
                                & crate::condition::bit(crate::condition::INVISIBLE)
                                == 0)
                })
                .map(|p| p.pos)
                .chain(self.decoy_positions())
                .min_by(|a, b| pos.distance(*a).total_cmp(&pos.distance(*b)));
            let wake = match kind {
                0x609 | 0xd06 | crate::dungeon::abyss::MALPHAS => 11.,
                0x617 | 0x61a | 0x6d8 => 12.,
                0x91a => 7.,
                crate::dungeon::spider::BOSS => 12.,
                0xd90 | 0xd7e => 8.,
                0xd82 | 0xd83 => 10.,
                _ => 20.,
            };
            let radius = if entity.activated.is_some() {
                20.
            } else {
                wake
            };
            let target = if burst {
                Some(pos)
            } else {
                nearest.filter(|p| pos.distance(*p) < radius)
            };
            let nearby = self.players.values().any(|p| {
                p.character.alive
                    && !p.death_pending
                    && !p.departing
                    && !p.paused
                    && pos.distance(p.pos) < radius
            });
            if !burst && target.is_none() && !nearby {
                self.suspend_npc(id, delta);
                continue;
            }
            let activated = *self
                .entities
                .get_mut(&id)
                .unwrap()
                .activated
                .get_or_insert(self.now);
            let elapsed = self.now - activated;
            if let Err(error) = self.encounter_mechanics(id, target, elapsed) {
                tracing::error!(entity=id, kind, %error, "encounter mechanic refused");
                continue;
            }
            let Some(current) = self.entities.get(&id) else {
                continue;
            };
            let elapsed = self.now - current.activated.expect("activated encounter");
            let flags = current.conditions(self.now);
            let (mut motion, mut speed) = if kind == 0x909 && current.stage == Some(1) {
                (Motion::Stationary, 0.)
            } else if crate::dungeon::trench::ROSTER.contains(&kind) {
                self.ocean_movement(id, target, elapsed)
            } else if crate::encounter::landmarks::ROSTER.contains(&kind) {
                match self.landmark_movement(id, target, elapsed) {
                    Ok(movement) => movement,
                    Err(error) => {
                        tracing::error!(entity=id,%error,"landmark movement refused");
                        continue;
                    }
                }
            } else if current.quest.is_some() {
                self.quest_movement(id, target, elapsed)
            } else if crate::encounter::fauna::ROSTER.contains(&kind) {
                match self.fauna_movement(id, target, elapsed) {
                    Ok(movement) => movement,
                    Err(error) => {
                        tracing::error!(entity=id,%error,"fauna movement refused");
                        continue;
                    }
                }
            } else if kind == 0x6c6 || (self.entities[&id].camp.is_some() && midlands::member(kind))
            {
                match self.midland_movement(id, target, elapsed) {
                    Ok(movement) => movement,
                    Err(error) => {
                        tracing::error!(entity=id,%error,"inland family movement refused");
                        continue;
                    }
                }
            } else if gods::GODS.contains(&kind)
                || gods::CONSTRUCTS.contains(&kind)
                || (kind == 0x653 && self.entities[&id].camp.is_some())
            {
                match self.god_movement(id, target, elapsed) {
                    Ok(movement) => movement,
                    Err(error) => {
                        tracing::error!(entity=id,%error,"Godlands movement refused");
                        continue;
                    }
                }
            } else if golems::ROOTS.contains(&kind) || matches!(kind, 0x6d2..=0x6d7) {
                match self.golem_movement(id, elapsed) {
                    Ok(movement) => movement,
                    Err(error) => {
                        tracing::error!(entity=id,%error,"golem movement refused");
                        continue;
                    }
                }
            } else if self.entities[&id].camp.is_some() {
                match self.family_movement(id, target, elapsed) {
                    Ok(movement) => movement,
                    Err(error) => {
                        tracing::error!(entity=id,%error,"family movement refused");
                        continue;
                    }
                }
            } else if matches!(kind, 0xdaa | 0xdac) {
                match self.lair_escort_movement(id) {
                    Ok(movement) => movement,
                    Err(error) => {
                        tracing::error!(entity=id,%error,"Lair escort movement refused");
                        continue;
                    }
                }
            } else if crate::endgame::ROSTER.contains(&kind) {
                match self.finale_movement(id, target, elapsed) {
                    Ok(movement) => movement,
                    Err(error) => {
                        tracing::error!(entity=id,%error,"endgame movement refused");
                        continue;
                    }
                }
            } else {
                crate::encounter::movement(kind, self.entities[&id].hp, elapsed)
            };
            if target.is_none()
                && matches!(
                    motion,
                    Motion::Chase { .. }
                        | Motion::Maintain { .. }
                        | Motion::Retreat { .. }
                        | Motion::Orbit {
                            anchor: crate::encounter::Anchor::Player,
                            ..
                        }
                        | Motion::Protect {
                            anchor: crate::encounter::Anchor::Player,
                            ..
                        }
                )
            {
                motion = Motion::Wander;
                speed = 4.;
            }
            let e = self.entities.get_mut(&id).unwrap();
            if e.motion != Some(motion) {
                e.motion = Some(motion);
                e.following = Following::Unknown;
                e.charge = None;
                e.goal = None;
                if matches!(motion, Motion::Pace { .. }) {
                    e.phase = 1.;
                }
                tracing::debug!(entity = id, kind, ?motion, speed, "NPC movement entered");
            }
            let planned_motion = if flags & crate::condition::bit(crate::condition::PARALYZED) != 0
            {
                if let Some(charge) = &mut e.charge {
                    charge.suspend(delta);
                }
                Motion::Stationary
            } else {
                motion
            };
            let goal = match planned_motion {
                Motion::Stationary => {
                    speed = 0.;
                    Some(pos)
                }
                Motion::Charge { .. } => self.charge_goal(id, target, motion, speed),
                Motion::WanderHome { radius } => {
                    let e = self.entities.get_mut(&id).unwrap();
                    if e.goal.is_none_or(|p| {
                        p.distance(e.pos) < 0.1 || p.distance(e.home) > radius + 0.01
                    }) {
                        let angle = self.rng.gen_range(0. ..std::f32::consts::TAU);
                        let distance = self.rng.gen_range(0. ..radius);
                        e.goal = Some(Pos {
                            x: e.home.x + distance * angle.cos(),
                            y: e.home.y + distance * angle.sin(),
                        });
                    }
                    e.goal
                }
                Motion::Follow { sight, stop, .. } => {
                    let e = self.entities.get_mut(&id).unwrap();
                    if flags & crate::condition::bit(crate::condition::PARALYZED) != 0 {
                        Some(pos)
                    } else {
                        let old = e.following;
                        let target = target.filter(|p| pos.distance(*p) < sight);
                        e.following = match target {
                            None => Following::Unknown,
                            Some(p)
                                if pos.distance(p)
                                    > stop + if old == Following::Resting { 1. } else { 0. } =>
                            {
                                Following::Acquired
                            }
                            Some(_) => Following::Resting,
                        };
                        if old != e.following {
                            e.goal = None;
                        }
                        if e.following == Following::Acquired {
                            target
                        } else {
                            None
                        }
                    }
                }
                Motion::Pace { distance } => {
                    let e = self.entities.get_mut(&id).unwrap();
                    Some(match e.goal {
                        Some(goal) if pos.distance(goal) > 0.001 => goal,
                        _ => {
                            if e.goal.is_some() {
                                e.phase = -e.phase;
                            }
                            Pos {
                                x: pos.x + distance * e.phase,
                                y: pos.y,
                            }
                        }
                    })
                }
                Motion::Tangential => {
                    let step = self.entities[&id].last_step;
                    let length = step.x.hypot(step.y);
                    if length == 0. {
                        speed = 0.;
                        Some(pos)
                    } else {
                        Some(Pos {
                            x: pos.x + step.x / length,
                            y: pos.y + step.y / length,
                        })
                    }
                }
                Motion::Maintain { sight, distance }
                    if target.is_some_and(|p| pos.distance(p) < sight) =>
                {
                    let target = target.expect("acquired maintain-distance target");
                    let angle = (pos.y - target.y).atan2(pos.x - target.x);
                    Some(Pos {
                        x: target.x + distance * angle.cos(),
                        y: target.y + distance * angle.sin(),
                    })
                }
                Motion::Retreat { distance } => {
                    let target = target.expect("target-dependent retreat guarded");
                    if pos.distance(target) >= distance {
                        speed = 0.;
                    }
                    let angle = (pos.y - target.y).atan2(pos.x - target.x);
                    Some(Pos {
                        x: target.x + distance * angle.cos(),
                        y: target.y + distance * angle.sin(),
                    })
                }
                Motion::Protect { anchor, distance } => {
                    let leader = match self.orbit_anchor(id, anchor, target) {
                        Ok(leader) => leader,
                        Err(error) => {
                            tracing::error!(entity=id, %error, "protection anchor refused");
                            continue;
                        }
                    };
                    if pos.distance(leader) <= distance {
                        speed = 0.;
                    }
                    Some(leader)
                }
                Motion::Chase { sight, stop }
                    if target.is_some_and(|p| pos.distance(p) < sight) =>
                {
                    if pos.distance(target.expect("acquired chase target")) <= stop {
                        speed = 0.;
                    }
                    target
                }
                Motion::Orbit { anchor, radius } | Motion::StrictOrbit { anchor, radius } => {
                    let target = match self.orbit_anchor(id, anchor, target) {
                        Ok(target) => target,
                        Err(error) => {
                            tracing::error!(entity=id, %error, "orbit anchor refused");
                            continue;
                        }
                    };
                    let e = self.entities.get_mut(&id).unwrap();
                    let direction = if let Some(orbit) = e.orbit {
                        if pos.distance(target) > 0.001 {
                            e.phase = (pos.y - target.y).atan2(pos.x - target.x);
                        }
                        orbit.direction
                    } else {
                        if e.goal.is_none() {
                            e.phase = if matches!(motion, Motion::StrictOrbit { .. }) {
                                self.rng.gen_range(0. ..std::f32::consts::TAU)
                            } else {
                                (pos.y - target.y).atan2(pos.x - target.x)
                            };
                        }
                        1.
                    };
                    let angular_speed = if e.quest.is_some()
                        || e.orbit.is_some()
                        || matches!(motion, Motion::StrictOrbit { .. })
                    {
                        (if e.quest.is_some() || e.orbit.is_some() {
                            speed / 1.5
                        } else {
                            speed
                        }) * if flags & crate::condition::bit(crate::condition::SLOWED) != 0 {
                            0.5
                        } else {
                            1.
                        }
                    } else {
                        speed
                    };
                    if flags & crate::condition::bit(crate::condition::PARALYZED) == 0 {
                        e.phase = (e.phase + direction * angular_speed / radius * 0.05)
                            % std::f32::consts::TAU;
                    }
                    Some(Pos {
                        x: target.x + e.phase.cos() * radius,
                        y: target.y + e.phase.sin() * radius,
                    })
                }
                Motion::Circle => {
                    let boss = self
                        .actors()
                        .find(|e| e.kind == 0x927 && pos.distance(e.pos) < 10.)
                        .map(|e| e.pos);
                    boss.map(|boss| {
                        let e = self.entities.get_mut(&id).unwrap();
                        if e.goal.is_none() {
                            e.phase = self.rng.gen_range(0. ..std::f32::consts::TAU);
                        }
                        e.phase = (e.phase + profile.speed / 3. * 0.05) % std::f32::consts::TAU;
                        Pos {
                            x: boss.x + e.phase.cos() * 3.,
                            y: boss.y + e.phase.sin() * 3.,
                        }
                    })
                }
                _ => None,
            };
            let goal = if kind == 0xd06 && pos.distance(self.entities[&id].home) > 7. {
                speed = 7.8;
                Some(
                    self.orbit_anchor(id, crate::encounter::Anchor::Home, target)
                        .expect("live boss home"),
                )
            } else {
                goal
            };
            let goal = if let Some(goal) = goal {
                goal
            } else {
                let e = self.entities.get_mut(&id).unwrap();
                speed = match motion {
                    Motion::Wander | Motion::Maintain { .. } => speed,
                    Motion::Follow { idle, .. } | Motion::Charge { idle, .. } => idle,
                    Motion::Chase { .. } if kind == 0x600 => 4.,
                    Motion::Chase { .. } if !(0x687..=0x689).contains(&kind) => 4.,
                    Motion::Protect { .. } => profile.speed,
                    _ => 2.,
                };
                let realm = self.name == "Realm";
                if e.goal.is_none_or(|goal| {
                    goal.distance(e.pos) < 0.1
                        || if realm {
                            goal.distance(e.pos) > 1.01
                        } else {
                            goal.distance(e.home) > 2.01
                        }
                }) {
                    let angle = self.rng.gen_range(0. ..std::f32::consts::TAU);
                    let (center, radius) = if realm {
                        (e.pos, 1.)
                    } else {
                        (e.home, self.rng.gen_range(0. ..2.))
                    };
                    e.goal = Some(if realm && e.pos.distance(e.home) > 24. {
                        e.home
                    } else {
                        Pos {
                            x: center.x + angle.cos() * radius,
                            y: center.y + angle.sin() * radius,
                        }
                    });
                }
                e.goal.expect("wander destination assigned")
            };
            if flags & crate::condition::bit(crate::condition::PARALYZED) != 0 {
                speed = 0.;
            } else if flags & crate::condition::bit(crate::condition::SLOWED) != 0 {
                speed *= 0.5;
            }
            if flags & crate::condition::bit(crate::condition::PARALYZED) == 0 {
                self.entities.get_mut(&id).unwrap().goal = Some(goal);
            }
            let distance = if matches!(motion, Motion::StrictOrbit { .. }) && speed > 0. {
                pos.distance(goal)
            } else {
                speed / 1.5 * 0.05
            };
            if let Err(error) = self.move_entity(id, goal, distance) {
                let e = self.entities.get_mut(&id).unwrap();
                e.goal = None;
                e.last_step = Pos { x: 0., y: 0. };
                tracing::debug!(entity=id,%error,"NPC movement blocked; retargeting");
            }
            let e = &self.entities[&id];
            let schedule = if burst {
                None
            } else {
                match self.npc_attack_schedule(id, elapsed) {
                    Ok(schedule) => schedule,
                    Err(error) => {
                        tracing::error!(entity=id,kind,%error,"NPC attack schedule refused");
                        continue;
                    }
                }
            };
            if profile.interval == 0
                || e.activated.is_none()
                || (!burst
                    && ((schedule.is_none()
                        && (self.now < e.next_shot
                            || target.is_none_or(|p| e.pos.distance(p) > profile.range)))
                        || flags & crate::condition::bit(crate::condition::STUNNED) != 0))
            {
                continue;
            }
            let result = if let Some(schedule) = &schedule {
                self.scheduled_attacks(id, target, schedule)
            } else {
                self.legacy_enemy_shots(id, target.expect("legacy attack target acquired"), elapsed)
            };
            let shots = match result {
                Ok(shots) => shots,
                Err(error) => {
                    tracing::error!(entity=id, kind, %error, "NPC projectiles refused");
                    continue;
                }
            };
            let shots = if schedule.is_none()
                && flags & crate::condition::bit(crate::condition::DAZED) != 0
            {
                enemy_shots::dazed(shots)
            } else {
                shots
            };
            let interval = match kind {
                0x633 if elapsed % 10000 >= 5000 => 500,
                0x917 if !(1875..3750).contains(&self.entities[&id].hp) => 1000,
                0xd06 if elapsed % 43300 >= 32300 => 1300,
                0xd06 if elapsed % 43300 >= 12300 => 500,
                0xd90 if (elapsed / 13000) % 4 == 1 => 2000,
                0xd90 if (elapsed / 13000) % 4 >= 2 => 1000,
                _ => profile.interval,
            };
            if schedule.is_none() {
                self.entities.get_mut(&id).unwrap().next_shot = self.now + interval;
            }
            if shield_burst {
                self.entities.get_mut(&id).unwrap().stage = Some(3);
                tracing::debug!(entity = id, kind, "Shield Orc final command selected");
            }
            if self.bullets.len() + shots.len() > 2048 {
                tracing::warn!(
                    entity = id,
                    active = self.bullets.len(),
                    "NPC shot refused at global projectile limit"
                );
                continue;
            }
            let mut notices = Vec::with_capacity(shots.len());
            for (index, angle) in shots {
                let spec = self.content.objects[&kind]
                    .projectiles
                    .get(&index)
                    .expect("selected encounter projectile missing")
                    .clone();
                let e = self.entities.get_mut(&id).unwrap();
                e.bullet = e.bullet.wrapping_add(1);
                let bullet = Bullet {
                    owner: id,
                    source_kind: kind,
                    id: e.bullet,
                    friendly: false,
                    start: e.pos,
                    pos: e.pos,
                    angle,
                    born: self.now,
                    damage: if spec.min == spec.max {
                        spec.min
                    } else {
                        self.rng.gen_range(spec.min..spec.max)
                    },
                    spec,
                    hit: BTreeSet::new(),
                };
                notices.push(enemy_shots::EnemyShot {
                    bullet: bullet.id,
                    projectile: index,
                    start: bullet.start,
                    angle,
                    damage: bullet.damage,
                });
                if kind == 0x927 {
                    tracing::info!(
                        boss = id,
                        projectile = index,
                        time_ms = self.now,
                        "Dreadstump projectile emitted"
                    );
                }
                self.bullets.push(bullet);
            }
            packets.extend(
                enemy_shots::packets(id, &notices)
                    .into_iter()
                    .map(|body| (id, body)),
            );
        }
        packets
    }
    pub fn step(&mut self) {
        let started = Instant::now();
        self.now += 50;
        self.tick += 1;
        self.expire_consumable_boosts();
        self.companion_step()
            .expect("validated companion simulation");
        self.trade_step();
        self.fountain_heal();
        self.expire_goto_acks();
        self.expire_native_projectiles();
        let ids: Vec<_> = self.players.keys().copied().collect();
        for id in &ids {
            let p = &self.players[id];
            if !p.active() || p.paused {
                continue;
            }
            let p = self.players.get_mut(id).unwrap();
            let values = p
                .effective(&self.content, self.now)
                .expect("validated player equipment and base stats");
            let flags = p.conditions(self.now);
            p.statuses
                .retain(|_, (_, deadline)| deadline.saturating_add(1000) > self.now);
            let regeneration = if flags & crate::condition::bit(crate::condition::BLEEDING) != 0 {
                0.
            } else {
                1. + values[6] as f32 / 8.
            } + if flags & crate::condition::bit(crate::condition::HEALING) != 0
            {
                20.
            } else {
                0.
            };
            if p.character.hp < values[0]
                && regeneration > 0.
                && flags & crate::condition::bit(crate::condition::SICK) == 0
            {
                p.hp_fraction += regeneration * 0.05;
                let n = p.hp_fraction as i32;
                p.character.hp = (p.character.hp + n).min(values[0]);
                p.hp_fraction -= n as f32;
            } else {
                p.hp_fraction = 0.;
            }
            if flags & crate::condition::bit(crate::condition::BLEEDING) != 0 {
                p.character.hp = (p.character.hp - 1).max(1);
            }
            if flags & crate::condition::bit(crate::condition::QUIET) != 0 {
                p.character.mp = 0;
            }
            if p.character.mp < values[1]
                && flags & crate::condition::bit(crate::condition::QUIET) == 0
            {
                p.mp_fraction += (0.6 + values[7] as f32 / 16.7) * 0.05;
                let n = p.mp_fraction as i32;
                p.character.mp = (p.character.mp + n).min(values[1]);
                p.mp_fraction -= n as f32;
            } else {
                p.mp_fraction = 0.;
            }
            let goal = p.bot_goal;
            let aim = p.bot_aim;
            if let Some(goal) = goal {
                let d = p.pos.distance(goal);
                if d > 0.02 {
                    let ground = self
                        .map
                        .cell(p.pos.x, p.pos.y)
                        .expect("player inside map")
                        .ground;
                    let step = p
                        .speed(&self.content, self.now)
                        .expect("valid player equipment")
                        * self.content.grounds[&ground]
                        * 50.;
                    let n = d.min(step);
                    let pos = Pos {
                        x: p.pos.x + (goal.x - p.pos.x) / d * n,
                        y: p.pos.y + (goal.y - p.pos.y) / d * n,
                    };
                    let time = self.now as i32;
                    if let Err(e) = self.move_to(*id, pos, time) {
                        tracing::debug!(player=id,error=%e,"bot movement blocked");
                    }
                }
            }
            if self.players[id].controller != Controller::Human {
                let pos = self.players[id].pos;
                let time = self.now as i32;
                if let Err(e) = self.move_to(*id, pos, time) {
                    tracing::error!(player=id,error=%e,"script idle MOVE rejected");
                }
            }
            if let Some(angle) = aim {
                let p = &self.players[id];
                let time = self.now as i32;
                let kind = p.character.items[0] as u16;
                let shots = self.content.objects[&kind].shots;
                let first = p.next_bullet;
                let pos = p.pos;
                let start = Pos {
                    x: pos.x + 0.3 * angle.cos(),
                    y: pos.y + 0.3 * angle.sin(),
                };
                for i in 0..shots {
                    if self
                        .shoot(*id, time, (first + i as u8) % 128, kind, start, angle)
                        .is_err()
                    {
                        break;
                    }
                }
            }
        }
        if let Err(error) = self.repopulate() {
            tracing::error!(%error,"Realm population replenishment failed");
        }
        let expired: Vec<_> = self
            .bag_expiry
            .iter()
            .filter(|(id, deadline)| {
                let bag = &self.entities[id];
                !bag.reserved
                    && (**deadline <= self.now
                        || (self.content.objects[&bag.kind].class == "Container"
                            && bag.items.iter().all(|item| *item == -1)))
            })
            .map(|(id, _)| *id)
            .collect();
        for id in expired {
            self.remove_entity(id);
            tracing::debug!(entity = id, "expired transient or empty container removed");
        }
        self.mechanic_expiry();
        self.abyss_step();
        self.ground_step();
        self.ocean_step();
        self.event_step();
        self.ability_step();
        let npc_started = Instant::now();
        let shots = self.npc_step();
        self.performance
            .borrow_mut()
            .record("npc", npc_started.elapsed());
        self.grenade_step();
        for id in &ids {
            if let Err(e) = self.updates(*id, false) {
                tracing::error!(player=id,error=%e,"world update failed");
            }
        }
        for (owner, shot) in shots {
            self.send_enemy_shot(owner, shot);
        }
        let guns: Vec<_> = self
            .actors()
            .filter(|e| (0x0802..=0x0805).contains(&e.kind) && self.now >= e.next_shot)
            .map(|e| e.id)
            .collect();
        for id in guns {
            let e = self.entities.get_mut(&id).unwrap();
            e.next_shot = self.now + self.rng.gen_range(2000..3500);
            e.bullet = e.bullet.wrapping_add(1);
            let angle = match e.kind {
                0x0802 => std::f32::consts::FRAC_PI_2,
                0x0804 => 0.,
                0x0805 => std::f32::consts::PI,
                _ => 0.,
            };
            let spec = self.content.objects[&e.kind]
                .projectiles
                .get(&0)
                .cloned()
                .expect("validated tutorial gun projectile zero");
            let b = Bullet {
                owner: id,
                source_kind: e.kind,
                id: e.bullet,
                friendly: false,
                start: e.pos,
                pos: e.pos,
                angle,
                born: self.now,
                damage: spec.min,
                spec,
                hit: BTreeSet::new(),
            };
            let mut o = Writer::new();
            o.u8(b.id)
                .i32(id)
                .u8(0)
                .f32(b.start.x)
                .f32(b.start.y)
                .f32(angle)
                .i16(b.damage as i16);
            self.bullets.push(b);
            self.send_enemy_shot(id, o.0);
        }
        let mut bullets = std::mem::take(&mut self.bullets);
        for b in &mut bullets {
            for sample in 1..=5 {
                if b.hit.contains(&-1) || (!b.spec.multi_hit && !b.hit.is_empty()) {
                    break;
                }
                let old = b.pos;
                b.pos = b.position(
                    (self.now - 50 + sample * 10)
                        .max(b.born)
                        .min(b.born + b.spec.lifetime as u64),
                );
                if self
                    .map
                    .cell(b.pos.x, b.pos.y)
                    .is_none_or(|c| c.ground == 255)
                    || self.entities_at(b.pos).any(|e| {
                        let o = &self.content.objects[&e.kind];
                        o.static_object
                            && (!o.enemy || !b.friendly)
                            && (o.enemy_occupy_square || o.occupy_square && !b.spec.passes_cover)
                    })
                {
                    b.hit.insert(-1);
                    break;
                }
                if !b.friendly
                    && !b.spec.multi_hit
                    && self.decoy_positions().any(|pos| inside_box(pos, b.pos))
                {
                    b.hit.insert(-1);
                    break;
                }
                if b.friendly {
                    let targets: Vec<_> = self
                        .near_entities(old, b.pos)
                        .filter(|e| {
                            e.hp > 0
                                && e.conditions(self.now)
                                    & (crate::condition::bit(crate::condition::STASIS)
                                        | crate::condition::bit(crate::condition::INVINCIBLE))
                                    == 0
                                && self.content.objects[&e.kind].destructible
                                && !b.hit.contains(&e.id)
                                && inside_box(e.pos, b.pos)
                        })
                        .map(|e| e.id)
                        .collect();
                    if let Some(target) = targets.iter().min_by(|a, b_id| {
                        self.entities[a]
                            .pos
                            .distance(b.pos)
                            .total_cmp(&self.entities[b_id].pos.distance(b.pos))
                    }) {
                        if self.players.get(&b.owner).is_some_and(|p| p.paused) {
                            b.hit.insert(*target);
                            break;
                        }
                        self.damage_enemy(
                            b.owner,
                            *target,
                            b.damage,
                            b.spec.armor_piercing,
                            &b.spec.effects,
                            Some(b.id),
                        )
                        .expect("validated enemy damage");
                        b.hit.insert(*target);
                    }
                } else if !self.safe {
                    for id in &ids {
                        let p = &self.players[id];
                        if p.character.alive
                            && !p.paused
                            && !p.departing
                            && !p.death_pending
                            && p.controller != Controller::Human
                            && !b.hit.contains(id)
                            && inside_box(p.pos, b.pos)
                        {
                            self.damage_player(*id, b, true)
                                .expect("validated projectile damage");
                            b.hit.insert(*id);
                            if !b.spec.multi_hit {
                                break;
                            }
                        }
                    }
                }
            }
        }
        bullets.retain(|b| {
            self.now - b.born < b.spec.lifetime as u64
                && !b.hit.contains(&-1)
                && (b.spec.multi_hit || b.hit.is_empty())
        });
        self.bullets = bullets;
        if self.tick.is_multiple_of(2) {
            for id in ids {
                if let Err(e) = self.updates(id, false) {
                    tracing::error!(player=id,error=%e,"visibility update failed");
                    continue;
                }
                let p = &self.players[&id];
                let dynamic: Vec<_> = p
                    .known
                    .iter()
                    .filter(|entity| {
                        self.players.contains_key(entity)
                            || self.companions.contains(entity)
                            || self.entities.get(entity).is_some_and(|e| {
                                e.hp > 0
                                    || !e.items.is_empty()
                                    || self.content.objects[&e.kind].class == "Portal"
                            })
                    })
                    .copied()
                    .collect();
                let mut o = Writer::new();
                o.i32(p.tick).i32(100).i16(dynamic.len() as i16);
                for entity in dynamic {
                    o.0.extend(self.stats(entity, id).expect("visible entity must exist"));
                }
                send(p, wire::NEW_TICK, o.0);
                let p = self.players.get_mut(&id).unwrap();
                p.remember_weapon_tick();
                let slow = crate::condition::bit(crate::condition::SLOWED);
                let paralyzed = crate::condition::bit(crate::condition::PARALYZED);
                let motion_condition = p.conditions(self.now) & (slow | paralyzed | SPEEDY);
                let base = 0.004
                    + 0.0056
                        * p.effective(&self.content, self.now)
                            .expect("valid player equipment")[4] as f32
                        / 75.;
                let speed = |flags: i32| {
                    if flags & paralyzed != 0 {
                        0.
                    } else if flags & slow != 0 {
                        0.004
                    } else {
                        base * if flags & SPEEDY != 0 { 1.5 } else { 1. }
                    }
                };
                let restricts_motion = speed(motion_condition) < speed(p.native_motion_condition);
                if p.controller == Controller::Human && p.active() && restricts_motion {
                    // The client must consume its real stat29 before acknowledging the new motor baseline.
                    let mut out = Writer::new();
                    out.i32(id).f32(p.pos.x).f32(p.pos.y);
                    reconciliation::goto(p, true, out.0);
                    tracing::info!(
                        player = id,
                        previous = p.native_motion_condition,
                        current = motion_condition,
                        "native movement condition synchronized"
                    );
                }
                p.native_motion_condition = motion_condition;
                p.tick += 1;
            }
        }
        let mut performance = self.performance.borrow_mut();
        performance.record("step", started.elapsed());
        if let Some(timings) = performance.report() {
            tracing::info!(world=%self.name, entities=self.entities.len(), actors=self.actor_ids.len(),
                humans=self.players.values().filter(|p| p.controller==Controller::Human).count(),
                bots=self.players.values().filter(|p| p.controller==Controller::Autonomous).count(),
                qa_actors=self.players.values().filter(|p| p.controller==Controller::Qa).count(),
                discovered_tiles=self.players.values().map(|p|p.tiles.len()).sum::<usize>(),
                cached_view_tiles=self.players.values().map(|p|p.view_tiles.borrow().tiles.len()).sum::<usize>(),
                timings=%timings,"world performance");
        }
    }
}
fn inside_box(a: Pos, b: Pos) -> bool {
    (a.x - b.x).abs() <= 0.5 && (a.y - b.y).abs() <= 0.5
}
#[cfg(test)]
#[path = "../examples/testing/cover.rs"]
mod cover_example;
#[cfg(test)]
mod gravestones_tests;
#[cfg(test)]
mod loot_interactions_tests;
#[cfg(test)]
#[path = "../examples/testing/timing.rs"]
mod timing_example;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn active_lifecycle_is_independent_of_pause_and_pending_writes() {
        let mut w = world();
        let id = w.enter(ch(&w), None).unwrap();
        let p = w.players.get_mut(&id).unwrap();
        p.paused = true;
        p.pending = true;
        for flags in 0..16 {
            p.character.alive = flags & 1 == 0;
            p.death_pending = flags & 2 != 0;
            p.departing = flags & 4 != 0;
            p.persistence_failure = (flags & 8 != 0).then(|| "uncertain commit".into());
            assert_eq!(p.active(), flags == 0, "lifecycle flags={flags}");
        }
    }
    #[test]
    fn prepared_worlds_share_explicit_seed_without_changing_normal_creation() {
        let content = Arc::new(Content::load(std::path::Path::new("data")).unwrap());
        let first =
            Directory::test_prepared(std::path::Path::new("data"), content.clone(), 4, 12351)
                .unwrap();
        let second =
            Directory::test_prepared(std::path::Path::new("data"), content.clone(), 4, 12351)
                .unwrap();
        for id in [-1, -2, -3, -4] {
            let mut a = first.worlds[&id].rng.clone();
            let mut b = second.worlds[&id].rng.clone();
            assert_eq!(
                rand::RngCore::next_u64(&mut a),
                rand::RngCore::next_u64(&mut b)
            );
            assert!(first.worlds[&id].prepared_seed.is_some());
        }
        let ordinary =
            Directory::test_with_seed(std::path::Path::new("data"), content, 4, 12351).unwrap();
        assert!(ordinary.worlds.values().all(|w| w.prepared_seed.is_none()));
    }
    use crate::content::WIZARD;
    use std::path::Path;
    #[test]
    fn spawned_nexus_exit_publishes_the_name_required_by_native_portal_draw() {
        let mut w = world();
        let id = w
            .spawn_entity(
                0x712,
                Pos {
                    x: w.map.spawns[0].0,
                    y: w.map.spawns[0].1,
                },
                vec![],
            )
            .unwrap();
        assert_eq!(w.entities[&id].name, "Nexus Portal");
        let bytes = w.stats(id, 0).unwrap();
        let mut r = crate::wire::Reader::new(&bytes);
        assert_eq!(r.i32().unwrap(), id);
        r.f32().unwrap();
        r.f32().unwrap();
        let count = r.i16().unwrap();
        let mut names = Vec::new();
        for _ in 0..count {
            let stat = r.u8().unwrap();
            if stat == 31 {
                names.push(r.utf().unwrap());
            } else {
                r.i32().unwrap();
            }
        }
        r.done().unwrap();
        assert_eq!(names, ["Nexus Portal"]);
    }
    pub(super) fn world() -> World {
        let c = Arc::new(Content::load(Path::new("data")).unwrap());
        let m = Arc::new(Map::load(Path::new("data/tutorial.wmap"), &c).unwrap());
        World::new(c, m).unwrap()
    }
    pub(super) fn arena(terrain: u8) -> World {
        let mut w = world();
        w.encounter = true;
        for id in w.entities.keys().copied().collect::<Vec<_>>() {
            w.remove_entity(id);
        }
        let map = Arc::make_mut(&mut w.map);
        map.spawns = vec![(1.5, 1.5)];
        for cell in &mut map.cells {
            cell.ground = 0x61;
            cell.terrain = terrain;
            cell.object = None;
        }
        w
    }
    pub(super) fn ch(w: &World) -> Character {
        Character {
            schema: 1,
            class: WIZARD,
            base: w.content.classes[&WIZARD].base,
            tex1: 0,
            tex2: 0,
            pet_kind: None,
            id: 1,
            account: 1,
            name: "Test".into(),
            level: 1,
            exp: 0,
            xp_boost: Default::default(),
            fame: 0,
            hp: 100,
            mp: 100,
            items: w.content.objects[&WIZARD].equipment.clone(),
            revision: 0,
            alive: true,
            created_at: 1,
            shots: 0,
            hits: 0,
            kills: 0,
            quests: 0,
            tiles: 0,
            pirate_caves: 0,
            dungeons: [0; 3],
            abysses: 0,
            trenches: 0,
            spider_dens: 0,
        }
    }
    pub(super) fn receive(
        d: &mut Directory,
        world: i32,
        account: i32,
        class: u16,
    ) -> (i32, mpsc::Receiver<Packet>) {
        let w = d.worlds.get_mut(&world).unwrap();
        let mut c = super::tests::ch(w);
        c.id = account + 100;
        c.account = account;
        c.class = class;
        c.base = w.content.classes[&class].base;
        c.items = w.content.objects[&class].equipment.clone();
        let (packets, mut rx) = mpsc::channel(128);
        let (lost, _) = watch::channel(false);
        let id = w.enter(c, Some(Output { packets, lost })).unwrap();
        while rx.try_recv().is_ok() {}
        (id, rx)
    }
    pub(super) fn directory() -> Directory {
        Directory::load(
            Path::new("data"),
            Arc::new(Content::load(Path::new("data")).unwrap()),
            16,
        )
        .unwrap()
    }
    #[test]
    fn directory_routes_and_global_identities_do_not_alias() {
        let mut d = directory();
        assert_eq!(d.initial(0).unwrap(), -2);
        for game in [-1, -2, -3, -4] {
            assert_eq!(d.initial(game).unwrap(), game);
        }
        assert!(d.initial(-5).is_err());
        assert!(d.initial(-6).is_err());
        let mut ids = BTreeSet::new();
        for world in d.worlds.values() {
            for id in world.entities.keys() {
                assert!(ids.insert(*id));
            }
        }
        let a = d.worlds.get_mut(&-1).unwrap();
        let first = a.enter(ch(a), None).unwrap();
        let b = d.worlds.get_mut(&-2).unwrap();
        let second = b.enter(ch(b), None).unwrap();
        assert_ne!(first, second);
        assert_eq!(d.location(first).unwrap(), -1);
        assert_eq!(d.location(second).unwrap(), -2);
        assert!(d.worlds[&-2].safe && !d.worlds[&-3].safe);
        assert!(d.worlds[&-2].entities.values().any(|e| e.kind == 0x712));
    }
    #[test]
    fn transfer_grants_are_bound_one_use_expiring_and_private() {
        let mut d = directory();
        let vault = d.instance("vault", Some(1)).unwrap();
        assert_eq!(d.instance("vault", Some(1)).unwrap(), vault);
        assert_ne!(d.instance("vault", Some(2)).unwrap(), vault);
        assert!(d.issue(2, 1, vault).is_err());
        let (time, key, _) = d.issue(1, 42, vault).unwrap();
        assert!(d.consume(2, vault, time, key.clone()).is_err());
        assert!(d.consume(1, -2, time, key.clone()).is_err());
        assert!(d.consume(1, vault, time + 1, key.clone()).is_err());
        assert_eq!(d.consume(1, vault, time, key.clone()).unwrap(), 42);
        assert!(d.consume(1, vault, time, key).is_err());
        let (time, key, _) = d.issue(1, 42, -2).unwrap();
        d.tick += 2400;
        assert!(d.consume(1, -2, time, key).is_err());
        let ids = AtomicI32::new(i32::MAX);
        assert!(allocate_id(&ids).is_err());
        assert_eq!(ids.load(Ordering::Relaxed), i32::MAX);
    }
    #[test]
    fn directory_instances_are_bounded_and_idle_instances_are_collected() {
        let mut d = directory();
        for _ in 0..8 {
            d.instance("pirate-cave", None).unwrap();
        }
        assert!(d.instance("pirate-cave", None).is_err());
        d.tick = 6001;
        d.step();
        assert_eq!(d.worlds.len(), 4);
    }
    #[test]
    fn sourced_worlds_preserve_connected_wall_and_merchant_config_stats() {
        let content = Arc::new(Content::load(Path::new("data")).unwrap());
        for name in [
            "nexus",
            "nexus-limbo",
            "vault",
            "realm-shore",
            "pirate-cave",
        ] {
            let map = Arc::new(
                Map::load(&Path::new("data").join(format!("{name}.wmap")), &content).unwrap(),
            );
            let world = World::new(content.clone(), map).unwrap();
            for e in world.entities.values() {
                world.stats(e.id, 0).unwrap();
            }
            if name == "pirate-cave" {
                assert!(
                    world
                        .entities
                        .values()
                        .any(|e| e.extra_stats.iter().any(|(stat, _)| *stat == 41))
                );
            }
            if name == "nexus" {
                let changer = world.entities.values().find(|e| e.kind == 0x1dc).unwrap();
                assert_eq!(changer.extra_stats, [(55, 0)]);
                assert!(
                    world
                        .entities
                        .values()
                        .any(|e| e.extra_stats.iter().any(|(stat, _)| *stat == 34))
                );
            }
        }
    }
    #[test]
    fn progress_is_invisible_until_commit_and_cannot_heal_pending_death() {
        let mut w = world();
        let id = w.enter(ch(&w), None).unwrap();
        w.players.get_mut(&id).unwrap().experience_queued = 50;
        let plans = w.pending_progress();
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].1.level, 2);
        assert_eq!(w.players[&id].character.level, 1);
        assert!(w.pending_progress().is_empty());
        let p = w.players.get_mut(&id).unwrap();
        p.character.hp = 0;
        p.death_pending = true;
        assert!(w.pending_deaths().is_empty());
        w.finish_progress(id, plans[0].1.clone()).unwrap();
        assert_eq!(w.players[&id].character.level, 2);
        assert_eq!(w.players[&id].character.hp, 0);
        let death = w.pending_deaths();
        assert_eq!(death.len(), 1);
        assert_eq!(death[0].1.level, 2);
        assert_eq!(death[0].1.hp, 0);
    }
    #[test]
    fn committed_fame_survives_departure_and_permanent_death() {
        let mut w = world();
        let id = w.enter(ch(&w), None).unwrap();
        w.players.get_mut(&id).unwrap().experience_queued = 1000;
        let plans = w.pending_progress();
        assert_eq!(plans[0].1.fame, 1);
        assert_eq!(w.players[&id].character.fame, 0);
        w.finish_progress(id, plans[0].1.clone()).unwrap();
        let saved = w.begin_departure(id).unwrap().unwrap();
        assert_eq!(saved.fame, 1);
        w.finish_departure(id).unwrap();
        let id = w.enter(saved, None).unwrap();
        let p = w.players.get_mut(&id).unwrap();
        p.character.hp = 0;
        p.death_pending = true;
        assert_eq!(w.pending_deaths()[0].1.fame, 1);
    }
    #[test]
    fn departure_waits_for_earned_progress() {
        let mut w = world();
        let id = w.enter(ch(&w), None).unwrap();
        w.players.get_mut(&id).unwrap().experience_queued = 50;
        assert!(w.begin_departure(id).unwrap().is_none());
        let plans = w.pending_progress();
        w.finish_progress(id, plans[0].1.clone()).unwrap();
        let leaving = w.begin_departure(id).unwrap().unwrap();
        assert_eq!(leaving.exp, 50);
        assert_eq!(leaving.level, 2);
        w.finish_departure(id).unwrap();
        assert!(!w.players.contains_key(&id));
    }
    #[test]
    fn health_excludes_qa_actors_from_population() {
        let mut d = directory();
        let w = d.worlds.get_mut(&-1).unwrap();
        w.enter(ch(w), None).unwrap();
        let mut native = ch(w);
        native.id = 2;
        native.account = 2;
        let (packets, _rx) = mpsc::channel(64);
        let (lost, _watch) = watch::channel(false);
        w.enter(native, Some(Output { packets, lost })).unwrap();
        let health = d.health(Instant::now());
        assert_eq!(health["players"], 1);
        assert_eq!(health["bots"], 0);
        assert_eq!(health["qa_actors"], 1);
        assert_eq!(health["worlds"], d.worlds.len());
        assert_eq!(
            health["entities"],
            d.worlds.values().map(|w| w.entities.len()).sum::<usize>()
        );
        assert_eq!(
            health["projectiles"],
            d.worlds.values().map(|w| w.bullets.len()).sum::<usize>()
        );
        assert_eq!(health["tick"], d.tick);
        assert_eq!(health["max_tick_us"], d.max_tick_us);
    }
    #[test]
    fn health_uptime_uses_elapsed_process_time_not_world_ticks() {
        let w = directory();
        let started = Instant::now();
        let before = w.health(started)["uptime_seconds"].as_u64().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let after = w.health(started)["uptime_seconds"].as_u64().unwrap();
        assert!(after > before);
        assert_eq!(w.tick, 0);
    }
    #[test]
    fn one_owner_and_no_teleport() {
        let mut w = world();
        let c = ch(&w);
        let id = w.enter(c.clone(), None).unwrap();
        assert!(w.enter(c, None).is_err());
        assert!(w.move_to(id, Pos { x: 12., y: 3. }, 100).is_err());
    }
    #[test]
    fn queue_loss_is_observable() {
        let mut w = world();
        let id = w.enter(ch(&w), None).unwrap();
        let (packets, mut rx) = mpsc::channel(1);
        let (lost, watch) = watch::channel(false);
        let p = w.players.get_mut(&id).unwrap();
        p.tx = Some(Output { packets, lost });
        send(p, 6, vec![1]);
        assert!(!*watch.borrow());
        send(p, 6, vec![2]);
        assert!(*watch.borrow());
        assert_eq!(rx.try_recv().unwrap().body, vec![1]);
    }
    #[test]
    fn control_cannot_steal_browser_input() {
        let mut w = world();
        let (packets, _rx) = mpsc::channel(64);
        let (lost, _watch) = watch::channel(false);
        let id = w.enter(ch(&w), Some(Output { packets, lost })).unwrap();
        assert!(
            w.action(id, Action::Stop)
                .err()
                .unwrap()
                .to_string()
                .contains("QA-owned")
        );
    }
    #[test]
    fn selected_collision_box_is_inclusive() {
        let a = Pos { x: 0., y: 0. };
        assert!(inside_box(a, Pos { x: 0.5, y: 0.5 }));
        assert!(inside_box(a, Pos { x: -0.5, y: -0.5 }));
        assert!(!inside_box(a, Pos { x: 0.500001, y: 0. }));
    }
    #[test]
    fn fog_reveals_circular_tiles_once_and_remembers_them() {
        let mut d = directory();
        let (id, mut rx) = receive(&mut d, -2, 1, WIZARD);
        let w = d.worlds.get_mut(&-2).unwrap();
        w.players.get_mut(&id).unwrap().tiles.clear();
        let initial_count = w.players[&id].character.tiles;
        let mut known = BTreeSet::new();
        for (cx, cy) in [(50, 50), (60, 50), (0, 0), (141, 129), (50, 50)] {
            w.players.get_mut(&id).unwrap().pos = Pos {
                x: cx as f32 + 0.25,
                y: cy as f32 + 0.75,
            };
            let circle: BTreeSet<_> = (0..w.map.height)
                .flat_map(|y| (0..w.map.width).map(move |x| (x, y)))
                .filter(|(x, y)| (x - cx).pow(2) + (y - cy).pow(2) <= 400)
                .map(|(x, y)| (x as i16, y as i16))
                .collect();
            let expected: BTreeSet<_> = circle.difference(&known).copied().collect();
            w.updates(id, false).unwrap();
            let mut received = BTreeSet::new();
            while let Ok(packet) = rx.try_recv() {
                if packet.id == wire::UPDATE {
                    let mut r = crate::wire::Reader::new(&packet.body);
                    for _ in 0..r.i16().unwrap() {
                        let (x, y) = (r.i16().unwrap(), r.i16().unwrap());
                        assert_eq!(
                            r.u8().unwrap(),
                            w.map.cells[y as usize * w.map.width as usize + x as usize].ground
                        );
                        assert!(received.insert((x, y)), "duplicate tile in UPDATE");
                    }
                }
            }
            assert_eq!(received, expected);
            known.extend(circle);
            assert_eq!(w.players[&id].tiles, known);
            assert_eq!(
                w.players[&id].character.tiles - initial_count,
                known.len() as u64
            );
            assert!(w.players[&id].known.contains(&id));
        }
    }
    #[test]
    fn native_minimap_keeps_distant_players_without_revealing_perception_or_fog() {
        let mut d = directory();
        let (observer, mut rx) = receive(&mut d, -3, 1, WIZARD);
        let w = d.worlds.get_mut(&-3).unwrap();
        let mut character = ch(w);
        character.id = 2;
        character.account = 2;
        let (packets, _rx) = mpsc::channel(128);
        let (lost, _) = watch::channel(false);
        let peer = w
            .enter_autonomous(character, Output { packets, lost })
            .unwrap();
        w.updates(observer, false).unwrap();
        assert!(w.players[&observer].known.contains(&peer));
        let origin = w.players[&observer].pos;
        let distant = (0..w.map.height)
            .flat_map(|y| {
                (0..w.map.width).map(move |x| Pos {
                    x: x as f32 + 0.5,
                    y: y as f32 + 0.5,
                })
            })
            .find(|pos| origin.distance(*pos) > 40. && w.walkable(*pos))
            .unwrap();
        w.players.get_mut(&peer).unwrap().pos = distant;
        while rx.try_recv().is_ok() {}
        w.updates(observer, false).unwrap();
        assert!(w.players[&observer].known.contains(&peer));
        assert!(!w.visible(observer).contains(&peer));
        assert!(
            !w.players[&observer]
                .tiles
                .contains(&(distant.x as i16, distant.y as i16))
        );
        assert!(w.entities.values().any(|e| origin.distance(e.pos) > 20.));
        let quest = w.quest_target(observer).unwrap();
        assert!(origin.distance(w.entities[&quest].pos) > 20.);
        assert!(w.entities.values().all(|e| {
            origin.distance(e.pos) < 20.
                || e.id == quest
                || !w.players[&observer].known.contains(&e.id)
        }));
        let private = w.loot_bag(origin, Some(2), vec![0xa4f]).unwrap();
        w.updates(observer, false).unwrap();
        assert!(!w.players[&observer].known.contains(&private));
        w.step();
        w.step();
        let expected = w.stats(peer, observer).unwrap();
        let mut tick = None;
        while let Ok(packet) = rx.try_recv() {
            if packet.id == wire::NEW_TICK {
                tick = Some(packet);
            }
        }
        assert!(
            tick.unwrap()
                .body
                .windows(expected.len())
                .any(|bytes| bytes == expected)
        );
        for (actor, hidden) in [(observer, peer), (peer, observer)] {
            let view = d.player_view(actor).unwrap();
            assert!(!view.objects.iter().any(|o| o.id == hidden));
            if actor == observer {
                assert!(view.objects.iter().any(|o| o.id == quest));
                let target = view
                    .objects
                    .iter()
                    .find(|o| o.id == quest)
                    .unwrap()
                    .position;
                assert!(
                    !view
                        .tiles
                        .iter()
                        .any(|t| t.x == target.x as i16 && t.y == target.y as i16)
                );
            }
        }
        let (newcomer, _) = receive(&mut d, -3, 3, WIZARD);
        assert!(d.worlds[&-3].players[&newcomer].known.contains(&peer));
        let (foreign, _) = receive(&mut d, -2, 4, WIZARD);
        let w = d.worlds.get_mut(&-3).unwrap();
        w.updates(observer, false).unwrap();
        assert!(!w.players[&observer].known.contains(&foreign));
        while rx.try_recv().is_ok() {}
        w.leave(peer).unwrap();
        w.updates(observer, false).unwrap();
        assert!(!w.players[&observer].known.contains(&peer));
        let update = rx.try_recv().unwrap();
        assert_eq!(update.id, wire::UPDATE);
        let mut r = crate::wire::Reader::new(&update.body);
        assert_eq!(r.i16().unwrap(), 0);
        assert_eq!(r.i16().unwrap(), 0);
        assert_eq!(r.i16().unwrap(), 1);
        assert_eq!(r.i32().unwrap(), peer);
        r.done().unwrap();
    }
    #[test]
    fn initial_update_places_self_before_private_container_owner_stats() {
        let mut w = world();
        let pos = Pos {
            x: w.map.spawns[0].0,
            y: w.map.spawns[0].1,
        };
        let bag = w.loot_bag(pos, Some(1), vec![0xa4f]).unwrap();
        let (tx, mut rx) = mpsc::channel(64);
        let (lost, _lost) = watch::channel(false);
        let id = w.enter(ch(&w), Some(Output { packets: tx, lost })).unwrap();
        assert!(bag < id);
        assert_eq!(rx.try_recv().unwrap().id, wire::CREATE_SUCCESS);
        let update = rx.try_recv().unwrap();
        assert_eq!(update.id, wire::UPDATE);
        let mut r = crate::wire::Reader::new(&update.body);
        let tiles = r.i16().unwrap();
        r.take(tiles as usize * 5).unwrap();
        assert!(r.i16().unwrap() >= 2);
        assert_eq!(r.i16().unwrap() as u16, WIZARD);
        assert_eq!(r.i32().unwrap(), id);
        assert!(w.visible(id).contains(&bag));
    }
    #[test]
    fn immutable_scenery_is_not_rebroadcast_in_dynamic_ticks() {
        let mut w = world();
        let (tx, mut rx) = mpsc::channel(64);
        let (lost, _lost) = watch::channel(false);
        let id = w.enter(ch(&w), Some(Output { packets: tx, lost })).unwrap();
        while rx.try_recv().is_ok() {}
        assert!(w.players[&id].known.iter().any(|entity| {
            w.entities
                .get(entity)
                .is_some_and(|e| e.hp == 0 && e.items.is_empty())
        }));
        w.step();
        w.step();
        let dynamic: Vec<_> = w.players[&id]
            .known
            .iter()
            .filter(|entity| {
                w.players.contains_key(entity)
                    || w.entities
                        .get(entity)
                        .is_some_and(|e| e.hp > 0 || !e.items.is_empty())
            })
            .copied()
            .collect();
        let mut expected = Writer::new();
        expected.i32(0).i32(100).i16(dynamic.len() as i16);
        for entity in dynamic {
            expected.0.extend(w.stats(entity, id).unwrap());
        }
        let mut tick = None;
        while let Ok(packet) = rx.try_recv() {
            if packet.id == wire::NEW_TICK {
                assert!(tick.replace(packet).is_none());
            }
        }
        assert_eq!(tick.unwrap().body, expected.0);
    }
    #[test]
    fn movement_cannot_borrow_future_clock() {
        let mut w = world();
        let id = w.enter(ch(&w), None).unwrap();
        let p = w.players[&id].pos;
        w.move_to(
            id,
            Pos {
                x: p.x + 0.4,
                y: p.y,
            },
            100,
        )
        .unwrap();
        assert!(
            w.move_to(
                id,
                Pos {
                    x: p.x + 0.8,
                    y: p.y
                },
                200
            )
            .is_err()
        );
    }
    #[test]
    fn shots_cannot_borrow_future_clock() {
        let mut w = world();
        let id = w.enter(ch(&w), None).unwrap();
        let p = w.players[&id].pos;
        let start = Pos {
            x: p.x + 0.3,
            y: p.y,
        };
        for (time, bullet) in [(100, 1), (100, 2), (500, 3), (500, 4)] {
            w.shoot(id, time, bullet, 0xa97, start, 0.).unwrap();
        }
        assert!(w.shoot(id, 900, 5, 0xa97, start, 0.).is_err());
        assert_eq!(w.bullets.len(), 4);
    }
    #[test]
    fn magic_potion_publishes_after_commit_and_cannot_restore_pending_death() {
        let mut w = world();
        let mut c = ch(&w);
        c.items[4] = 0xa23;
        c.mp = 25;
        let id = w.enter(c, None).unwrap();
        let slot = Slot {
            object: id,
            slot: 4,
            kind: 0xa23,
        };
        let plan = w.use_item(id, slot, w.players[&id].pos).unwrap().unwrap();
        assert_eq!(w.players[&id].character.mp, 25);
        assert_eq!(w.players[&id].character.items[4], 0xa23);
        w.finish_inventory(plan, Ok(1)).unwrap();
        assert_eq!(w.players[&id].character.mp, 100);
        assert_eq!(w.players[&id].character.items[4], -1);
        let p = w.players.get_mut(&id).unwrap();
        p.character.items[4] = 0xa23;
        p.character.mp = 25;
        let plan = w.use_item(id, slot, w.players[&id].pos).unwrap().unwrap();
        let p = w.players.get_mut(&id).unwrap();
        p.character.hp = 0;
        p.death_pending = true;
        w.finish_inventory(plan, Ok(2)).unwrap();
        assert_eq!(w.players[&id].character.mp, 25);
    }
    #[test]
    fn selected_container_put_flags_cannot_expose_soulbound_items() {
        let mut w = world();
        let soulbound = w
            .content
            .objects
            .values()
            .find(|o| o.class == "Equipment" && o.soulbound && o.bag_type <= 4)
            .unwrap()
            .kind;
        let mut c = ch(&w);
        c.items[5] = i32::from(soulbound);
        let id = w.enter(c, None).unwrap();
        let pos = w.players[&id].pos;
        let public = w.loot_bag(pos, None, vec![0xa22]).unwrap();
        let from = Slot {
            object: id,
            slot: 5,
            kind: i32::from(soulbound),
        };
        assert!(
            w.swap(
                id,
                from,
                Slot {
                    object: public,
                    slot: 1,
                    kind: -1
                }
            )
            .is_err()
        );
        let private = w
            .loot_bag(pos, Some(1), vec![i32::from(soulbound)])
            .unwrap();
        w.entities.get_mut(&private).unwrap().kind = 0x503;
        assert!(
            w.swap(
                id,
                Slot {
                    object: id,
                    slot: 4,
                    kind: 0xa22
                },
                Slot {
                    object: private,
                    slot: 1,
                    kind: -1
                }
            )
            .is_err()
        );
        w.entities.get_mut(&private).unwrap().kind = 0x504;
        let plan = w
            .swap(
                id,
                from,
                Slot {
                    object: private,
                    slot: 1,
                    kind: -1,
                },
            )
            .unwrap();
        assert_eq!(w.players[&id].character.items[5], i32::from(soulbound));
        w.finish_inventory(plan, Ok(1)).unwrap();
        assert_eq!(w.players[&id].character.items[5], -1);
        assert_eq!(w.entities[&private].items[1], i32::from(soulbound));
    }
    #[test]
    fn late_heal_cannot_revive() {
        let mut w = world();
        let id = w.enter(ch(&w), None).unwrap();
        let plan = w
            .use_item(
                id,
                Slot {
                    object: id,
                    slot: 4,
                    kind: 0xa22,
                },
                w.players[&id].pos,
            )
            .unwrap()
            .unwrap();
        let p = w.players.get_mut(&id).unwrap();
        p.character.hp = 0;
        p.death_pending = true;
        w.finish_inventory(plan, Ok(1)).unwrap();
        assert_eq!(w.players[&id].character.hp, 0);
        assert_eq!(w.players[&id].character.items[4], -1);
        let deaths = w.pending_deaths();
        assert_eq!(deaths.len(), 1);
        assert!(!deaths[0].1.alive);
        assert!(w.pending_deaths().is_empty());
        w.finish_death(id).unwrap();
        assert!(!w.players[&id].character.alive);
    }
    #[test]
    fn failed_pickup_quarantines_both_owners() {
        let mut w = world();
        let id = w.enter(ch(&w), None).unwrap();
        let bag = w
            .entities
            .values()
            .find(|e| e.kind == 0x806)
            .unwrap()
            .clone();
        w.players.get_mut(&id).unwrap().pos = bag.pos;
        let plan = w
            .swap(
                id,
                Slot {
                    object: bag.id,
                    slot: 0,
                    kind: 0xa4f,
                },
                Slot {
                    object: id,
                    slot: 3,
                    kind: -1,
                },
            )
            .unwrap();
        assert!(w.entities[&bag.id].reserved);
        assert!(
            w.finish_inventory(
                plan,
                Err(anyhow::anyhow!("synthetic storage failure").context("pickup commit")),
            )
            .is_err()
        );
        assert!(w.players[&id].pending && w.entities[&bag.id].reserved);
        assert_eq!(
            w.players[&id].persistence_failure.as_deref(),
            Some("pickup commit: synthetic storage failure")
        );
        assert!(w.begin_departure(id).is_err());
        assert_eq!(w.entities[&bag.id].items[0], 0xa4f);
        assert_eq!(w.players[&id].character.items[3], -1);
    }
    #[tokio::test]
    async fn sourced_private_chest_publishes_only_atomic_character_vault_commit() {
        use crate::store::{InventoryWrite, Store};
        let content = Arc::new(Content::load(std::path::Path::new("data")).unwrap());
        let path = std::env::temp_dir().join(format!(
            "vault-rearrangement-{:016x}.db",
            rand::random::<u64>()
        ));
        let store = Store::open(&path, &content).unwrap();
        let account = store.list("vault-actor".into()).await.unwrap();
        let ch = store
            .create(account.id, content.get(crate::content::WIZARD).unwrap())
            .await
            .unwrap();
        let map = Arc::new(Map::load(std::path::Path::new("data/vault.wmap"), &content).unwrap());
        let mut w = World::with_ids(content, map, Arc::new(AtomicI32::new(1))).unwrap();
        w.name = "Vault".into();
        w.owner = Some(account.id);
        w.safe = true;
        let original = store.vault(account.id).await.unwrap();
        let mut foreign = original.clone();
        foreign.account += 1;
        assert!(w.load_vault(foreign).is_err());
        w.load_vault(original.clone()).unwrap();
        let player = w.enter(ch, None).unwrap();
        let chest = w.vault.as_ref().unwrap().0;
        w.players.get_mut(&player).unwrap().pos = w.entities[&chest].pos;
        let plan = w
            .swap(
                player,
                Slot {
                    object: player,
                    slot: 4,
                    kind: 0xa22,
                },
                Slot {
                    object: chest,
                    slot: 0,
                    kind: -1,
                },
            )
            .unwrap();
        assert_eq!(w.entities[&chest].items[0], -1);
        assert_eq!(store.vault(account.id).await.unwrap().items[0], -1);
        let revisions = store
            .exchange(
                vec![InventoryWrite {
                    id: plan.character,
                    account: plan.account,
                    revision: plan.revision,
                    items: plan.items.clone(),
                }],
                plan.vault.clone(),
            )
            .await
            .unwrap();
        w.finish_inventory(plan, Ok(revisions[0])).unwrap();
        assert_eq!(w.players[&player].character.items[4], -1);
        assert_eq!(w.entities[&chest].items[0], 0xa22);
        let saved = store.vault(account.id).await.unwrap();
        assert_eq!(saved.items[0], 0xa22);
        assert_eq!(saved.revision, 1);
        assert!(w.load_vault(original).is_err());
        w.load_vault(saved).unwrap();
        for (from, to) in [
            ((player, 0), (chest, 1)),
            ((chest, 0), (chest, 2)),
            ((chest, 2), (chest, 1)),
        ] {
            let slot = |(object, slot)| Slot {
                object,
                slot,
                kind: if object == player {
                    w.players[&player].character.items[slot]
                } else {
                    w.entities[&chest].items[slot]
                },
            };
            let before = w.entities[&chest].items.clone();
            let inventory = w.players[&player].character.items.clone();
            let plan = w.swap(player, slot(from), slot(to)).unwrap();
            if from.0 == chest {
                assert_eq!(plan.items, inventory);
            }
            assert!(plan.collected.is_none() && plan.dropped.is_none());
            assert!(w.players[&player].pending && w.entities[&chest].reserved);
            assert!(
                w.swap(
                    player,
                    Slot {
                        object: chest,
                        slot: 0,
                        kind: before[0]
                    },
                    Slot {
                        object: chest,
                        slot: 1,
                        kind: before[1]
                    }
                )
                .is_err()
            );
            let revisions = store
                .exchange(
                    vec![InventoryWrite {
                        id: plan.character,
                        account: plan.account,
                        revision: plan.revision,
                        items: plan.items.clone(),
                    }],
                    plan.vault.clone(),
                )
                .await
                .unwrap();
            assert_eq!(w.entities[&chest].items, before);
            w.finish_inventory(plan, Ok(revisions[0])).unwrap();
            assert_eq!(
                w.entities[&chest].items,
                store.vault(account.id).await.unwrap().items
            );
        }
        let expected = vec![-1, 0xa22, 0xa97, -1, -1, -1, -1, -1];
        assert_eq!(w.entities[&chest].items, expected);
        assert_eq!(w.players[&player].character.revision, 4);
        let from = Slot {
            object: chest,
            slot: 1,
            kind: 0xa22,
        };
        let to = Slot {
            object: chest,
            slot: 2,
            kind: 0xa97,
        };
        let other = w
            .spawn_entity(0x504, w.entities[&chest].pos, vec![-1; 8])
            .unwrap();
        for (a, b) in [
            (
                from,
                Slot {
                    object: other,
                    slot: 0,
                    kind: -1,
                },
            ),
            (
                Slot {
                    object: other,
                    slot: 0,
                    kind: -1,
                },
                Slot {
                    object: other,
                    slot: 1,
                    kind: -1,
                },
            ),
            (Slot { kind: -1, ..from }, to),
            (from, Slot { kind: -1, ..to }),
            (Slot { slot: 8, ..from }, to),
            (from, Slot { slot: 8, ..to }),
        ] {
            assert!(w.swap(player, a, b).is_err());
            assert!(!w.players[&player].pending && !w.entities[&chest].reserved);
            assert_eq!(w.entities[&chest].items, expected);
        }
        let ownership = w.entities[&chest].extra_stats.clone();
        w.entities.get_mut(&chest).unwrap().extra_stats = vec![(54, account.id + 1)];
        assert!(w.swap(player, from, to).is_err());
        w.entities.get_mut(&chest).unwrap().extra_stats = ownership;
        w.players.get_mut(&player).unwrap().pos.x += 2.0;
        assert!(w.swap(player, from, to).is_err());
        w.players.get_mut(&player).unwrap().pos = w.entities[&chest].pos;
        let plan = w.swap(player, from, to).unwrap();
        let mut stale = plan.vault.clone().unwrap();
        stale.revision += 1;
        let failed = store
            .exchange(
                vec![InventoryWrite {
                    id: plan.character,
                    account: plan.account,
                    revision: plan.revision,
                    items: plan.items.clone(),
                }],
                Some(stale),
            )
            .await
            .unwrap_err();
        assert!(w.finish_inventory(plan, Err(failed)).is_err());
        assert!(w.players[&player].persistence_failure.is_some() && w.entities[&chest].reserved);
        assert_eq!(w.entities[&chest].items, expected);
        drop(store);
        let reopened = Store::open(&path, &w.content).unwrap();
        let vault = reopened.vault(account.id).await.unwrap();
        assert_eq!((vault.revision, vault.items), (4, expected));
        let character = reopened
            .load(account.id, w.players[&player].character.id)
            .await
            .unwrap();
        assert_eq!(character.revision, 4);
        assert_eq!(character.items, w.players[&player].character.items);
        drop(reopened);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn ground_drop_is_hidden_until_commit_and_quarantines_uncertain_failure() {
        let mut w = world();
        let owner = w.enter(ch(&w), None).unwrap();
        let mut second = ch(&w);
        second.id = 2;
        second.account = 2;
        let other = w.enter(second, None).unwrap();
        assert!(
            w.drop_item(
                owner,
                Slot {
                    object: other,
                    slot: 4,
                    kind: 0xa22
                }
            )
            .is_err()
        );
        let plan = w
            .drop_item(
                owner,
                Slot {
                    object: owner,
                    slot: 4,
                    kind: 0xa22,
                },
            )
            .unwrap();
        let ground = plan.dropped.unwrap();
        assert!(!w.visible(owner).contains(&ground));
        assert!(!w.visible(other).contains(&ground));
        assert_eq!(w.players[&owner].character.items[4], 0xa22);
        assert!(
            w.drop_item(
                owner,
                Slot {
                    object: owner,
                    slot: 4,
                    kind: 0xa22
                }
            )
            .is_err()
        );
        w.finish_inventory(plan, Ok(1)).unwrap();
        assert_eq!(w.players[&owner].character.items[4], -1);
        assert!(w.visible(other).contains(&ground));
        let pick = w
            .swap(
                other,
                Slot {
                    object: ground,
                    slot: 0,
                    kind: 0xa22,
                },
                Slot {
                    object: other,
                    slot: 5,
                    kind: -1,
                },
            )
            .unwrap();
        w.finish_inventory(pick, Ok(1)).unwrap();
        assert_eq!(w.entities[&ground].items[0], -1);
        assert_eq!(w.players[&other].character.items[5], 0xa22);
        let plan = w
            .drop_item(
                other,
                Slot {
                    object: other,
                    slot: 5,
                    kind: 0xa22,
                },
            )
            .unwrap();
        let uncertain = plan.dropped.unwrap();
        assert!(
            w.finish_inventory(plan, Err(anyhow::anyhow!("lost database result")))
                .is_err()
        );
        assert!(w.players[&other].pending);
        assert!(w.begin_departure(other).is_err());
        w.now += 60_000;
        w.step();
        assert!(w.entities[&uncertain].reserved);
        assert!(!w.visible(owner).contains(&uncertain));
        assert_eq!(w.players[&other].character.items[5], 0xa22);
    }
    #[test]
    fn empty_ground_bag_despawns_only_after_commit_before_timeout() {
        let mut w = world();
        let owner = w.enter(ch(&w), None).unwrap();
        let bag = w
            .loot_bag(w.players[&owner].pos, None, vec![0xa22])
            .unwrap();
        let plan = w
            .swap(
                owner,
                Slot {
                    object: bag,
                    slot: 0,
                    kind: 0xa22,
                },
                Slot {
                    object: owner,
                    slot: 5,
                    kind: -1,
                },
            )
            .unwrap();
        w.step();
        assert!(w.entities.contains_key(&bag));
        assert_eq!(w.players[&owner].character.items[5], -1);
        w.finish_inventory(plan, Ok(1)).unwrap();
        assert!(w.now < 60_000);
        w.step();
        assert!(!w.entities.contains_key(&bag));
        assert!(!w.bag_expiry.contains_key(&bag));
        assert!(w.cells.iter().all(|cell| !cell.contains(&bag)));
        assert_eq!(w.players[&owner].character.items[5], 0xa22);
    }
    #[test]
    fn private_loot_is_server_scoped_and_reservations_protect_expiry() {
        let mut w = world();
        let owner = w.enter(ch(&w), None).unwrap();
        let mut ch2 = ch(&w);
        ch2.id = 2;
        ch2.account = 2;
        let other = w.enter(ch2, None).unwrap();
        let pos = w.players[&owner].pos;
        let bag = w.loot_bag(pos, Some(1), vec![0xa4f]).unwrap();
        assert!(w.visible(owner).contains(&bag));
        assert!(!w.visible(other).contains(&bag));
        for observer in [Some(other), None] {
            assert!(
                !w.inspect_map(observer).unwrap()["entities"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|e| e["id"] == bag)
            );
        }
        assert!(
            w.inspect_map(Some(owner)).unwrap()["entities"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["id"] == bag)
        );
        assert!(w.inspect_map(Some(-999)).is_err());
        assert!(
            !w.observe(other).unwrap()["entities"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["id"] == bag)
        );
        let from = Slot {
            object: bag,
            slot: 0,
            kind: 0xa4f,
        };
        assert!(
            w.swap(
                other,
                from,
                Slot {
                    object: other,
                    slot: 5,
                    kind: -1
                }
            )
            .is_err()
        );
        let plan = w
            .swap(
                owner,
                from,
                Slot {
                    object: owner,
                    slot: 5,
                    kind: -1,
                },
            )
            .unwrap();
        w.now = 60_000;
        w.step();
        assert!(w.entities.contains_key(&bag));
        assert_eq!(w.players[&owner].character.items[5], -1);
        w.finish_inventory(plan, Ok(1)).unwrap();
        w.step();
        assert!(!w.entities.contains_key(&bag));
        assert_eq!(w.players[&owner].character.items[5], 0xa4f);
        assert_eq!(w.players[&other].character.items[5], -1);
        assert_eq!(w.cells.iter().filter(|c| c.contains(&bag)).count(), 0);
    }
    #[test]
    fn native_projectile_observation_exposes_only_the_players_friendly_origins() {
        let mut w = world();
        let a = w.enter(ch(&w), None).unwrap();
        let mut other = ch(&w);
        other.id = 2;
        other.account = 2;
        let b = w.enter(other, None).unwrap();
        w.now = 1000;
        for id in [a, b] {
            let p = w.players[&id].pos;
            w.shoot(
                id,
                1000,
                1,
                2711,
                Pos {
                    x: p.x + 0.3,
                    y: p.y,
                },
                0.,
            )
            .unwrap();
        }
        let mut hostile = w.bullets[0].clone();
        hostile.friendly = false;
        w.bullets.push(hostile);
        let observed = w.observe(a).unwrap();
        let own = observed["own_projectiles"].as_array().unwrap();
        assert_eq!(own.len(), 1);
        assert_eq!(
            own[0]["start"],
            serde_json::to_value(w.bullets[0].start).unwrap()
        );
        assert_eq!(own[0]["angle"], 0.);
    }
    #[test]
    fn native_spread_uses_central_muzzle_without_funding_false_centres() {
        let mut w = world();
        let player = w.enter(ch(&w), None).unwrap();
        let kind = 0xc03;
        let item = w.content.get(kind).unwrap();
        let shots = item.shots;
        let gap = item.arc.to_radians();
        assert_eq!(shots, 10);
        let p = w.players.get_mut(&player).unwrap();
        p.character.items[0] = i32::from(kind);
        p.next_bullet = 0;
        p.last_move = 1000;
        p.last_move_at = w.now;
        p.movement_credit = 0.;
        let pos = p.pos;
        let aim = 0.8f32;
        let muzzle = Pos {
            x: pos.x + 0.3 * aim.cos(),
            y: pos.y + 0.3 * aim.sin(),
        };
        for index in 0..shots {
            let angle = aim + gap * (index as f32 - (shots - 1) as f32 / 2.);
            if index == 1 {
                assert!(
                    w.shoot(player, 1000, index as u8, kind, muzzle, angle + 0.5)
                        .unwrap_err()
                        .to_string()
                        .contains("invalid shot origin")
                );
                assert_eq!(w.players[&player].attack_count, 1);
            }
            w.shoot(player, 1000, index as u8, kind, muzzle, angle)
                .unwrap();
        }
        assert_eq!(w.players[&player].attack_count, shots);
        assert_eq!(w.players[&player].movement_credit, 0.);
        assert_eq!(w.players[&player].pos.x, pos.x);
        assert_eq!(w.players[&player].pos.y, pos.y);
    }
    #[test]
    fn native_shoot_before_move_uses_the_same_speed_time_and_wall_budget() {
        let mut w = world();
        let id = w.enter(ch(&w), None).unwrap();
        let pos = (1..w.map.height - 1)
            .flat_map(|y| {
                (1..w.map.width - 1).map(move |x| Pos {
                    x: x as f32 + 0.5,
                    y: y as f32 + 0.5,
                })
            })
            .find(|p| {
                (-1..=1).all(|dy| {
                    (-1..=1).all(|dx| {
                        w.walkable(Pos {
                            x: p.x + dx as f32,
                            y: p.y + dy as f32,
                        })
                    })
                })
            })
            .unwrap();
        let p = w.players.get_mut(&id).unwrap();
        p.pos = pos;
        p.last_move = 1000;
        p.last_move_at = 0;
        p.movement_credit = 0.;
        w.now = 100;
        let origin = Pos {
            x: pos.x + 0.673777,
            y: pos.y - 0.139555,
        };
        let angle = (-0.139555_f32).atan2(0.673777);
        assert!(
            w.shoot(
                id,
                1091,
                1,
                0xa97,
                Pos {
                    x: pos.x + 20.,
                    y: pos.y
                },
                0.
            )
            .is_err()
        );
        w.shoot(id, 1091, 1, 0xa97, origin, angle).unwrap();
        assert_eq!(w.players[&id].pos.x, pos.x);
        assert_eq!(w.players[&id].last_move, 1000);
        assert_eq!(w.players[&id].movement_credit, 0.);
        assert_eq!(w.players[&id].character.shots, 1);
        assert!(
            w.validate_move(
                id,
                Pos {
                    x: pos.x + 0.6,
                    y: pos.y
                },
                1091
            )
            .is_err()
        );
        assert!(w.validate_move(id, pos, 999).is_err());
        assert!(w.move_to(id, pos, 2001).is_err());
        w.now = 5100;
        let stationary = Pos {
            x: pos.x + 0.3 * angle.cos(),
            y: pos.y + 0.3 * angle.sin(),
        };
        w.shoot(id, 6000, 2, 0xa97, stationary, angle).unwrap();
        w.now = 5600;
        w.shoot(
            id,
            6500,
            3,
            0xa97,
            Pos {
                x: pos.x + 0.35,
                y: pos.y,
            },
            0.,
        )
        .unwrap();
        assert_eq!(w.players[&id].last_move, 1000);
        assert_eq!(w.players[&id].movement_credit, 0.);
        assert!(
            w.shoot(
                id,
                7000,
                3,
                0xa97,
                Pos {
                    x: f32::NAN,
                    y: pos.y
                },
                angle
            )
            .is_err()
        );
        let blocker = *w.content.objects.iter().find(|(_, o)| o.blocks).unwrap().0;
        w.spawn_entity(blocker, pos, vec![]).unwrap();
        assert!(
            w.validate_move(
                id,
                Pos {
                    x: pos.x + 0.1,
                    y: pos.y
                },
                1091
            )
            .is_ok()
        );
        w.spawn_entity(
            blocker,
            Pos {
                x: pos.x + 1.,
                y: pos.y,
            },
            vec![],
        )
        .unwrap();
        assert!(
            w.validate_move(
                id,
                Pos {
                    x: pos.x + 0.6,
                    y: pos.y
                },
                1200
            )
            .unwrap_err()
            .to_string()
            .contains("blocked tile")
        );
    }
    #[test]
    fn hiscore_snapshots_exclude_qa_and_keep_the_latest_fame_until_death_commits() {
        let mut d = Directory::load(Path::new("data"), world().content, 16).unwrap();
        let world_id = d.initial(-2).unwrap();
        let (id, _receiver) = receive(&mut d, world_id, 1, WIZARD);
        let w = d.worlds.get_mut(&world_id).unwrap();
        w.players.get_mut(&id).unwrap().character.fame = 100;
        w.players.get_mut(&id).unwrap().death_pending = true;
        assert_eq!(d.competitive_characters()[0].fame, 100);
        d.worlds
            .get_mut(&world_id)
            .unwrap()
            .players
            .get_mut(&id)
            .unwrap()
            .controller = Controller::Qa;
        assert!(d.competitive_characters().is_empty());
        let p = d
            .worlds
            .get_mut(&world_id)
            .unwrap()
            .players
            .get_mut(&id)
            .unwrap();
        p.controller = Controller::Human;
        p.persistence_failure = Some("test storage failure".into());
        assert!(d.competitive_characters().is_empty());
    }
    #[test]
    fn only_human_enemy_drop_pickups_stage_collection_and_player_drops_never_do() {
        for controller in [Controller::Human, Controller::Qa, Controller::Autonomous] {
            let (mut w, id, enemy) = encounter(0x683);
            w.players.get_mut(&id).unwrap().controller = controller;
            w.rng = rand::rngs::StdRng::seed_from_u64(5);
            let mut defeated = w.remove_entity(enemy);
            defeated.damage.insert(w.players[&id].character.id, 100);
            for _ in 0..32 {
                w.drop_loot(&defeated).unwrap();
                if w.entities.values().any(|e| !e.drop_slots.is_empty()) {
                    break;
                }
            }
            let bag = w
                .entities
                .values()
                .find(|e| !e.drop_slots.is_empty())
                .unwrap()
                .clone();
            let slot = *bag.drop_slots.first().unwrap();
            let kind = bag.items[slot];
            w.players.get_mut(&id).unwrap().pos = bag.pos;
            let plan = w
                .swap(
                    id,
                    Slot {
                        object: bag.id,
                        slot,
                        kind,
                    },
                    Slot {
                        object: id,
                        slot: 11,
                        kind: -1,
                    },
                )
                .unwrap();
            assert_eq!(
                plan.collected,
                if controller == Controller::Human {
                    Some(kind as u16)
                } else {
                    None
                }
            );
            assert!(w.entities[&bag.id].drop_slots.contains(&slot));
            w.finish_inventory(plan, Ok(1)).unwrap();
            assert!(!w.entities[&bag.id].drop_slots.contains(&slot));
            let plan = w
                .drop_item(
                    id,
                    Slot {
                        object: id,
                        slot: 11,
                        kind,
                    },
                )
                .unwrap();
            let dropped = plan.dropped.unwrap();
            assert_eq!(plan.collected, None);
            w.finish_inventory(plan, Ok(2)).unwrap();
            let plan = w
                .swap(
                    id,
                    Slot {
                        object: dropped,
                        slot: 0,
                        kind,
                    },
                    Slot {
                        object: id,
                        slot: 11,
                        kind: -1,
                    },
                )
                .unwrap();
            assert_eq!(plan.collected, None);
        }
    }
    #[test]
    fn dungeon_clear_counter_is_staged_until_progress_commit() {
        let (mut w, player, boss) = encounter(0x927);
        w.name = "Pirate Cave".into();
        let mut defeated = w.remove_entity(boss);
        defeated.damage.insert(w.players[&player].character.id, 101);
        w.drop_loot(&defeated).unwrap();
        assert!(w.cleared);
        assert_eq!(w.players[&player].character.pirate_caves, 0);
        assert!(w.entities.values().any(|e| e.kind == 0x71d && !e.published));
        assert!(!w.entities.values().any(|e| e.kind == 0x71d && e.published));
        assert!(w.drop_loot(&defeated).is_err());
        let plans = w.pending_progress();
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].1.pirate_caves, 1);
        assert!(w.pending_progress().is_empty());
        assert!(w.begin_departure(player).unwrap().is_none());
        w.finish_progress(player, plans[0].1.clone()).unwrap();
        assert_eq!(w.players[&player].character.pirate_caves, 1);
        assert!(w.entities.values().any(|e| e.kind == 0x71d && e.published));
        assert!(w.clear_exit.is_none());
    }
    fn encounter(kind: u16) -> (World, i32, i32) {
        let mut w = world();
        let obsolete: Vec<_> = w
            .entities
            .values()
            .filter(|e| w.content.objects[&e.kind].enemy)
            .map(|e| e.id)
            .collect();
        for id in obsolete {
            w.remove_entity(id);
        }
        let player = w.enter(ch(&w), None).unwrap();
        let pos = w.players[&player].pos;
        let npc = w
            .spawn_entity(
                kind,
                Pos {
                    x: pos.x + 4.,
                    y: pos.y,
                },
                vec![],
            )
            .unwrap();
        w.encounter = true;
        (w, player, npc)
    }
    #[test]
    fn dropped_portal_binds_both_players_to_one_cave_and_expires() {
        let mut d = Directory::load(Path::new("data"), world().content, 16).unwrap();
        let w = d.worlds.get_mut(&-3).unwrap();
        let pos = w.population[0].pos;
        let a = w.enter(ch(w), None).unwrap();
        let mut other = ch(w);
        other.id = 2;
        other.account = 2;
        let b = w.enter(other, None).unwrap();
        w.players.get_mut(&a).unwrap().pos = pos;
        w.players.get_mut(&b).unwrap().pos = pos;
        let portal = d.drop_portal(-3, pos).unwrap();
        assert_eq!(d.portals[&portal].expires, d.tick + 600);
        d.worlds
            .get_mut(&-3)
            .unwrap()
            .entities
            .get_mut(&portal)
            .unwrap()
            .published = false;
        assert!(
            d.destination(a, Some(portal))
                .unwrap_err()
                .to_string()
                .contains("portal not published")
        );
        d.worlds
            .get_mut(&-3)
            .unwrap()
            .entities
            .get_mut(&portal)
            .unwrap()
            .published = true;
        let (_, _, destination) = d.destination(a, Some(portal)).unwrap();
        assert_eq!(d.destination(b, Some(portal)).unwrap().2, destination);
        assert_eq!(d.worlds[&destination].name, "Pirate Cave");
        d.worlds
            .get_mut(&-3)
            .unwrap()
            .players
            .get_mut(&b)
            .unwrap()
            .pos = Pos {
            x: pos.x + 3.,
            y: pos.y,
        };
        assert!(d.destination(b, Some(portal)).is_err());
        let fake = d
            .worlds
            .get_mut(&-3)
            .unwrap()
            .spawn_entity(0x717, pos, vec![])
            .unwrap();
        assert!(d.destination(a, Some(fake)).is_err());
        d.tick = 599;
        d.step();
        assert!(!d.portals.contains_key(&portal));
        assert!(!d.worlds[&-3].entities.contains_key(&portal));
        assert!(d.destination(a, Some(portal)).is_err());
    }
    #[test]
    fn sprite_world_portal_stays_usable_for_fifteen_seconds_then_expires() {
        let mut d = Directory::load(Path::new("data"), world().content, 16).unwrap();
        let w = d.worlds.get_mut(&-3).unwrap();
        let pos = w.population[0].pos;
        let player = w.enter(ch(w), None).unwrap();
        w.players.get_mut(&player).unwrap().pos = pos;
        d.tick = 1234;
        let born = d.tick;
        let portal = d.drop_dungeon_portal(-3, 0x70c, pos).unwrap();
        for age in [200, 299] {
            d.tick = born + age - 1;
            d.step();
            assert!(d.worlds[&-3].entities.contains_key(&portal));
            let destination = d.destination(player, Some(portal)).unwrap().2;
            assert_eq!(d.worlds[&destination].name, "Sprite World");
        }
        d.step();
        assert_eq!(d.tick - born, 300);
        assert!(!d.portals.contains_key(&portal));
        assert!(!d.worlds[&-3].entities.contains_key(&portal));
        assert!(d.destination(player, Some(portal)).is_err());
    }
    #[test]
    fn shore_population_is_bounded_sourced_and_cannot_respawn_on_players() {
        let mut d = Directory::load(Path::new("data"), world().content, 16).unwrap();
        let w = d.worlds.get_mut(&-3).unwrap();
        assert_eq!(w.population_count(11), 24);
        for spawn in w
            .population
            .iter()
            .filter(|s| w.content.objects[&s.kind].terrain >= 11)
        {
            assert_eq!(
                w.map.cell(spawn.pos.x, spawn.pos.y).unwrap().terrain,
                w.content.objects[&spawn.kind].terrain
            );
            assert!(w.walkable(spawn.pos));
        }
        let slot = w.population.iter().position(|s| s.kind == 0x602).unwrap();
        let pos = w.population[slot].pos;
        let old = w.population[slot].entity.unwrap();
        w.remove_entity(old);
        w.repopulate().unwrap();
        let player = w.enter(ch(w), None).unwrap();
        w.players.get_mut(&player).unwrap().pos = pos;
        w.now = 60_000;
        w.repopulate().unwrap();
        assert!(w.population[slot].entity.is_none());
        w.players.get_mut(&player).unwrap().pos = Pos { x: 0.5, y: 0.5 };
        w.repopulate().unwrap();
        assert_ne!(w.population[slot].entity.unwrap(), old);
        assert_eq!(w.entities_at(pos).filter(|e| e.kind == 0x602).count(), 1);
        assert_eq!(
            w.entities
                .values()
                .filter(|e| {
                    let object = &w.content.objects[&e.kind];
                    object.enemy
                        && object.class != "Wall"
                        && !(e.quest.is_some() && matches!(e.kind, 0xd5e | 0xd65))
                })
                .count(),
            w.population
                .iter()
                .filter(|s| s.entity.is_some_and(|id| w.entities.contains_key(&id)))
                .count()
                + w.hero_identities().len()
        );
        assert!(w.population_count(11) <= 24);
        assert_eq!(w.hero_identities().len(), 14);
        assert!(
            w.population
                .iter()
                .all(|s| !crate::endgame::HEROES.contains(&s.kind))
        );
    }
    #[test]
    fn moving_chaser_stays_indexed_and_fires_real_selected_projectiles() {
        let (mut w, player, npc) = encounter(0x687);
        let initial = w.entities[&npc].pos;
        for _ in 0..20 {
            w.step();
        }
        let current = w.entities[&npc].pos;
        assert!(initial.distance(current) > 2.);
        assert!(current.distance(w.players[&player].pos) <= 1.3);
        assert_eq!(w.cells.iter().filter(|cell| cell.contains(&npc)).count(), 1);
        assert!(w.entities_at(current).any(|e| e.id == npc));
        assert!(w.players[&player].character.hp < 100);
        let before = current;
        assert!(w.move_entity(npc, Pos { x: -10., y: -10. }, 100.).is_err());
        assert_eq!(w.entities[&npc].pos.distance(before), 0.);
        assert!(w.entities_at(before).any(|e| e.id == npc));
    }
    #[test]
    fn boss_emits_both_attacks_only_after_threshold_and_safe_worlds_do_not_run_ai() {
        let (mut w, _, boss) = encounter(0x927);
        w.step();
        assert!(!w.bullets.iter().any(|b| b.owner == boss));
        w.entities.get_mut(&boss).unwrap().hp = 639;
        for _ in 0..10 {
            w.step();
        }
        let attacks: Vec<_> = w
            .bullets
            .iter()
            .filter(|b| b.owner == boss)
            .map(|b| b.spec.speed)
            .collect();
        assert!(attacks.contains(&75.));
        assert!(attacks.contains(&130.));
        let pos = w.entities[&boss].pos;
        w.safe = true;
        for _ in 0..40 {
            w.step();
        }
        assert_eq!(w.entities[&boss].pos.distance(pos), 0.);
        assert!(!w.bullets.iter().any(|b| b.owner == boss));
    }
    #[test]
    fn overlapping_entities_preserve_collision_visibility_and_removal() {
        let mut w = world();
        let (x, y) = w.map.spawns[0];
        let pos = Pos { x, y };
        let first = w.spawn_entity(0x806, pos, vec![-1; 8]).unwrap();
        let second = w.spawn_entity(0x806, pos, vec![-1; 8]).unwrap();
        assert_eq!(w.entities_at(pos).count(), 2);
        assert_eq!(
            w.near_entities(pos, pos)
                .filter(|e| e.id == first || e.id == second)
                .count(),
            2
        );
        let wall = w.spawn_entity(0x163, pos, vec![]).unwrap();
        assert!(!w.walkable(pos));
        assert!(w.spawn_entity(0x806, pos, vec![]).is_err());
        w.remove_entity(wall);
        assert!(w.walkable(pos));
        w.remove_entity(first);
        assert_eq!(
            w.entities_at(pos).map(|e| e.id).collect::<Vec<_>>(),
            vec![second]
        );
    }
    #[test]
    fn projectile_keeps_killer_identity_after_emitter_dies() {
        let mut w = world();
        let mut character = ch(&w);
        character.hp = 1;
        let player = w.enter(character, None).unwrap();
        let emitter = w.entities.values().find(|e| e.kind == 0x802).unwrap().id;
        let kind = w.entities[&emitter].kind;
        let pos = w.players[&player].pos;
        w.bullets.push(Bullet {
            owner: emitter,
            source_kind: kind,
            id: 1,
            friendly: false,
            start: pos,
            pos,
            angle: 0.,
            born: 0,
            damage: 100,
            spec: w.content.objects[&kind].projectiles[&0].clone(),
            hit: BTreeSet::new(),
        });
        w.remove_entity(emitter);
        w.step();
        assert!(w.players[&player].death_pending);
        assert_eq!(w.players[&player].killer, w.content.objects[&kind].name);
    }
    #[test]
    fn selected_cover_stops_hostile_projectiles_before_player_damage() {
        for (kind, passes_cover, hp) in [
            (0x120, false, 100),
            (0x120, true, 100),
            (0x100, false, 100),
            (0x100, true, 92),
        ] {
            let mut w = world();
            for id in w.entities.keys().copied().collect::<Vec<_>>() {
                w.remove_entity(id);
            }
            for cell in &mut Arc::make_mut(&mut w.map).cells {
                cell.ground = 0x61;
            }
            let player = w.enter(ch(&w), None).unwrap();
            w.players.get_mut(&player).unwrap().pos = Pos { x: 12.5, y: 100.5 };
            w.spawn_entity(kind, Pos { x: 11.5, y: 100.5 }, vec![])
                .unwrap();
            let start = Pos { x: 10.5, y: 100.5 };
            let mut spec = w.content.objects[&0x683].projectiles[&0].clone();
            spec.passes_cover = passes_cover;
            w.bullets.push(Bullet {
                owner: 500,
                source_kind: 0x683,
                id: 1,
                friendly: false,
                start,
                pos: start,
                angle: 0.,
                born: 0,
                damage: 8,
                spec,
                hit: BTreeSet::new(),
            });
            for _ in 0..5 {
                w.step();
            }
            assert_eq!(
                w.players[&player].character.hp, hp,
                "{kind:#x}/{passes_cover}"
            );
            assert!(w.bullets.is_empty());
        }
    }
    #[test]
    fn native_movement_and_shot_origin_follow_client_positions_without_funding_scripts() {
        let mut w = world();
        let player = w.enter(ch(&w), None).unwrap();
        let pos = w.players[&player].pos;
        w.attack_epoch = Instant::now() - std::time::Duration::from_millis(2000);
        let p = w.players.get_mut(&player).unwrap();
        p.last_move = 1000;
        p.last_move_at = 1000;
        p.movement_credit = 0.;
        w.now = 1000;
        let next = Pos {
            x: pos.x + 0.4,
            y: pos.y,
        };
        assert!(w.move_to(player, next, 1100).is_err());
        w.native_move_to(player, next, 1100).unwrap();
        let received = w.players[&player].last_move_at;
        assert!(received >= 2000);
        let predicted = Pos {
            x: next.x + 0.4,
            y: next.y,
        };
        assert!(w.move_timed(player, predicted, 1200, received).is_err());
        w.shoot_timed(
            player,
            (1200, received, true),
            1,
            0xa97,
            Pos {
                x: predicted.x + 0.3,
                y: predicted.y,
            },
            0.,
        )
        .unwrap();
        assert_eq!(w.players[&player].movement_credit, 0.);
        assert_eq!(w.players[&player].pos.x, next.x);
        assert_eq!(w.bullets.len(), 1);
    }

    #[test]
    fn native_positions_preserve_actual_ingress_without_rejecting_actor_or_network_delay() {
        let mut w = world();
        let player = w.enter(ch(&w), None).unwrap();
        let pos = w.players[&player].pos;
        w.attack_epoch = Instant::now() - std::time::Duration::from_millis(2000);
        let received = w.attack_epoch + std::time::Duration::from_millis(1100);
        let p = w.players.get_mut(&player).unwrap();
        p.last_move = 1000;
        p.last_move_at = 1000;
        p.movement_credit = 0.;
        let next = Pos {
            x: pos.x + 0.7,
            y: pos.y,
        };
        w.native_move_received(player, next, (1100, received))
            .unwrap();
        w.native_shoot_received(
            player,
            (1100, received),
            1,
            0xa97,
            Pos {
                x: next.x + 0.3,
                y: next.y,
            },
            0.,
        )
        .unwrap();
        let p = &w.players[&player];
        assert_eq!((p.last_move_at, p.last_attack_at), (1100, 1100));
        assert_eq!(p.pos.x, next.x);
        assert_eq!(p.movement_credit, 0.);
        assert!(
            w.native_move_received(
                player,
                next,
                (1200, received - std::time::Duration::from_millis(1))
            )
            .is_err()
        );
    }

    #[test]
    fn queued_progress_must_settle_before_pause_is_confirmed() {
        let mut w = world();
        let character = ch(&w);
        let player = w.enter(character, None).unwrap();
        w.players.get_mut(&player).unwrap().experience_queued = 20;
        assert!(!w.settled().unwrap());
        let (id, progress) = w.pending_progress().pop().unwrap();
        assert!(!w.settled().unwrap());
        w.finish_progress(id, progress).unwrap();
        assert!(w.settled().unwrap());
    }
    #[test]
    fn paused_ingress_clock_stays_frozen_while_client_cadence_funds_native_shots() {
        let mut w = world();
        let mut character = ch(&w);
        character.base[5] = 26;
        let player = w.enter(character, None).unwrap();
        let pos = w.players[&player].pos;
        let start = Pos {
            x: pos.x + 0.3,
            y: pos.y,
        };
        let p = w.players.get_mut(&player).unwrap();
        p.last_attack = 1000;
        p.last_attack_at = 1000;
        p.attack_credit = 0.;
        w.attack_epoch = w.clock.now() - std::time::Duration::from_millis(1000);
        w.clock.set_paused(true);
        let paused = w.clock.now();
        std::thread::sleep(std::time::Duration::from_millis(400));
        assert_eq!(w.clock.now(), paused);
        assert_eq!(
            w.native_shoot_received(player, (1298, w.clock.now()), 1, 0xa97, start, 0.)
                .unwrap(),
            NativeInput::Applied
        );
        w.clock.set_paused(false);
        assert_eq!(
            w.native_shoot_received(player, (1596, w.clock.now()), 2, 0xa97, start, 0.)
                .unwrap(),
            NativeInput::Applied
        );
        assert!(w.players[&player].last_attack_at < 1100);
        assert_eq!(w.players[&player].character.shots, 2);
        assert_eq!(w.bullets.len(), 2);
    }
    #[test]
    fn native_attack_cadence_uses_client_time_but_keeps_real_ingress_metadata() {
        let mut w = world();
        let mut character = ch(&w);
        character.base[5] = 26;
        let player = w.enter(character, None).unwrap();
        let pos = w.players[&player].pos;
        let start = Pos {
            x: pos.x + 0.3,
            y: pos.y,
        };
        let p = w.players.get_mut(&player).unwrap();
        p.last_attack = 1000;
        p.last_attack_at = 1000;
        p.attack_credit = 0.;
        for (time, received) in [(1298, 1200), (1596, 1298)] {
            for _ in 0..2 {
                let bullet = w.players[&player].next_bullet;
                assert_eq!(
                    w.shoot_timed(player, (time, received, true), bullet, 0xa97, start, 0.)
                        .unwrap(),
                    NativeInput::Applied
                );
            }
        }
        w.now = 9000;
        w.shoot_timed(player, (1894, 1400, true), 5, 0xa97, start, 0.)
            .unwrap();
        assert_eq!(w.players[&player].character.shots, 5);
        assert_eq!(w.players[&player].last_attack_at, 1400);
        let seed = w.players[&player].seed;
        assert!(
            w.shoot_timed(player, (2192, 1200, true), 6, 0xa97, start, 0.)
                .unwrap_err()
                .to_string()
                .contains("attack receive clock regressed")
        );
        assert_eq!(w.players[&player].seed, seed);
    }
    #[test]
    fn native_integer_attack_period_keeps_the_bounded_server_credit() {
        for dexterity in [15, 17, 18, 25, 75] {
            let mut w = world();
            let mut character = ch(&w);
            character.base[5] = dexterity;
            let player = w.enter(character, None).unwrap();
            let period = (1. / (0.0015_f64 + (0.008 - 0.0015) * dexterity as f64 / 75.)) as i32;
            let pos = w.players[&player].pos;
            let start = Pos {
                x: pos.x + 0.3,
                y: pos.y,
            };
            for attack in 0..2000 {
                let time = attack * period;
                w.now = time as u64;
                for _ in 0..2 {
                    let bullet = w.players[&player].next_bullet;
                    w.shoot(player, time, bullet, 0xa97, start, 0.).unwrap();
                }
                w.bullets.clear();
            }
            assert_eq!(w.players[&player].character.shots, 4000);
            let bullet = w.players[&player].next_bullet;
            assert!(
                w.shoot(player, 2000 * period - 3, bullet, 0xa97, start, 0.)
                    .is_err()
            );
        }
    }
    #[test]
    fn candidate_tutorial_gun_still_reaches_its_open_lane() {
        let mut w = world();
        let player = w.enter(ch(&w), None).unwrap();
        w.players.get_mut(&player).unwrap().pos = Pos { x: 12.2, y: 137.8 };
        assert!((0..200).any(|_| {
            w.step();
            w.players[&player].character.hp < 100
        }));
        w.players.get_mut(&player).unwrap().pos = Pos { x: 12.2, y: 138.0 };
        assert!((0..3000).any(|_| {
            w.step();
            w.players[&player].death_pending
        }));
    }
    #[test]
    fn paired_staff_wave() {
        let w = world();
        let spec = w.content.objects[&0xa97].projectiles[&0].clone();
        let mut b = Bullet {
            owner: 1,
            source_kind: WIZARD,
            id: 1,
            friendly: true,
            start: Pos { x: 0., y: 0. },
            pos: Pos { x: 0., y: 0. },
            angle: 0.,
            born: 0,
            damage: 7,
            spec,
            hit: BTreeSet::new(),
        };
        let a = b.position(60);
        b.id = 2;
        let c = b.position(60);
        assert!((a.x - 1.08).abs() < 0.001);
        assert!((a.y + c.y).abs() < 0.001);
        assert!(a.y.abs() > 0.4);
    }
}
