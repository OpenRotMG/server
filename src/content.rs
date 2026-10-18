use anyhow::{Context, Result, bail, ensure};
use roxmltree::Node;
mod abilities;
pub use abilities::Ability;
mod consumables;
pub use consumables::{Companion, ConsumableEffect};
use serde::Serialize;
use std::{collections::BTreeMap, io::Read, path::Path};

pub const WIZARD: u16 = 0x030e;
pub const PRIEST: u16 = 0x0310;
#[cfg(test)]
pub const ARCHER: u16 = 0x0307;
#[cfg(test)]
pub const ROGUE: u16 = 0x0300;
#[cfg(test)]
pub const WARRIOR: u16 = 0x031d;
#[cfg(test)]
pub const KNIGHT: u16 = 0x031e;
#[cfg(test)]
pub const PALADIN: u16 = 0x031f;
pub const SLOWED: i32 = 1 << 3;
pub const STUNNED: i32 = 1 << 6;
pub const PARALYZED: i32 = 1 << 13;
pub const SPEEDY: i32 = 1 << 14;
pub const STATS: [(u8, &str); 8] = [
    (0, "MaxHitPoints"),
    (3, "MaxMagicPoints"),
    (20, "Attack"),
    (21, "Defense"),
    (22, "Speed"),
    (28, "Dexterity"),
    (26, "HpRegen"),
    (27, "MpRegen"),
];
#[derive(Clone, Debug)]
pub struct Class {
    pub base: [i32; 8],
    pub caps: [i32; 8],
    pub growth: [(i32, i32); 8],
    pub requires: Vec<(u16, i32)>,
}
#[derive(Clone, Debug, Serialize)]
pub struct Projectile {
    pub speed: f32,
    pub lifetime: f32,
    pub min: i32,
    pub max: i32,
    pub amplitude: f32,
    pub frequency: f32,
    pub passes_cover: bool,
    pub multi_hit: bool,
    pub armor_piercing: bool,
    pub wavy: bool,
    pub parametric: bool,
    pub boomerang: bool,
    pub magnitude: f32,
    pub effects: Vec<(u8, u64)>,
}
impl Projectile {
    pub fn offset(&self, id: u8, angle: f32, elapsed: f32) -> (f32, f32) {
        let parity = if id.is_multiple_of(2) {
            0.
        } else {
            std::f32::consts::PI
        };
        let mut distance = elapsed * self.speed / 10000.;
        if self.wavy {
            let direction = angle
                + std::f32::consts::PI / 64.
                    * (parity + 6. * std::f32::consts::PI * elapsed / 1000.).sin();
            return (distance * direction.cos(), distance * direction.sin());
        }
        if self.parametric {
            let phase = elapsed / self.lifetime * std::f32::consts::TAU;
            let a = phase.sin() * if id.is_multiple_of(2) { -1. } else { 1. };
            let b = (phase * 2.).sin() * if id % 4 < 2 { 1. } else { -1. };
            return (
                (a * angle.cos() - b * angle.sin()) * self.magnitude,
                (a * angle.sin() + b * angle.cos()) * self.magnitude,
            );
        }
        if self.boomerang && elapsed > self.lifetime / 2. {
            distance = (self.lifetime - elapsed) * self.speed / 10000.;
        }
        let wave = self.amplitude
            * (parity + std::f32::consts::TAU * self.frequency * elapsed / self.lifetime).sin();
        (
            distance * angle.cos() - wave * angle.sin(),
            distance * angle.sin() + wave * angle.cos(),
        )
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct Object {
    pub kind: u16,
    pub name: String,
    pub class: String,
    pub hp: i32,
    pub size: i32,
    pub mp: i32,
    pub attack: i32,
    pub defense: i32,
    pub speed: i32,
    pub dexterity: i32,
    pub vitality: i32,
    pub wisdom: i32,
    pub equipment: Vec<i32>,
    pub slots: Vec<i32>,
    pub slot_type: i32,
    pub tier: Option<u8>,
    pub soulbound: bool,
    pub bag_type: u8,
    pub put_normal: bool,
    pub put_soulbound: bool,
    pub bonuses: Vec<(u8, i32)>,
    pub unsupported_equipment: bool,
    pub projectiles: BTreeMap<u8, Projectile>,
    pub shots: i32,
    pub arc: f32,
    pub rate: f32,
    pub mana_cost: i32,
    pub activate: String,
    pub heal: i32,
    pub stat_increase: Option<(u8, i32)>,
    pub unlock_portal: Option<(String, String)>,
    pub create_object: Option<String>,
    pub dungeon_name: Option<String>,
    pub consumable: bool,
    pub resurrects: bool,
    pub textures: [Option<u32>; 2],
    pub successor: Option<String>,
    pub doses: Option<u8>,
    pub consumable_effects: Vec<ConsumableEffect>,
    pub xp_boost: bool,
    pub companion: Option<Companion>,
    pub heal_range: f32,
    pub activations: usize,
    pub abilities: Vec<Ability>,
    pub cooldown_ms: u64,
    pub enemy: bool,
    pub stasis_immune: bool,
    pub terrain: u8,
    pub quest: bool,
    pub quest_level: Option<i32>,
    pub xp_multiplier: f32,
    pub blocks: bool,
    pub full_occupy: bool,
    pub occupy_square: bool,
    pub enemy_occupy_square: bool,
    pub static_object: bool,
    pub protects_ground: bool,
    pub destructible: bool,
}
#[derive(Clone)]
pub struct Hazard {
    pub name: String,
    pub min: i32,
    pub max: i32,
}
#[derive(Clone)]
pub struct Content {
    pub objects: BTreeMap<u16, Object>,
    pub grounds: BTreeMap<u8, f32>,
    pub pushes: BTreeMap<u8, (f32, f32)>,
    pub hazards: BTreeMap<u8, Hazard>,
    pub classes: BTreeMap<u16, Class>,
}
fn child<'a, 'i>(n: Node<'a, 'i>, tag: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|c| c.has_tag_name(tag))
}
fn texture(n: Node<'_, '_>, tag: &str) -> Result<Option<u32>> {
    child(n, tag)
        .map(|_| {
            let value = text(n, tag);
            Ok(if let Some(hex) = value.strip_prefix("0x") {
                u32::from_str_radix(hex, 16)?
            } else {
                value.parse()?
            })
        })
        .transpose()
}
fn text(n: Node<'_, '_>, tag: &str) -> String {
    child(n, tag)
        .and_then(|c| c.text())
        .unwrap_or("")
        .trim()
        .to_owned()
}
// Omitted optional fields have the client's XML defaults, not substitute content.
fn number(n: Node<'_, '_>, tag: &str, default: f32) -> Result<f32> {
    match child(n, tag).and_then(|c| c.text()) {
        None => Ok(default),
        Some(v) => {
            let x: f32 = v.trim().parse().with_context(|| {
                format!("{} {tag}: {v}", n.attribute("id").unwrap_or("projectile"))
            })?;
            ensure!(x.is_finite(), "non-finite XML number");
            Ok(x)
        }
    }
}
fn list(n: Node<'_, '_>, tag: &str) -> Result<Vec<i32>> {
    let s = text(n, tag);
    if s.is_empty() {
        return Ok(vec![]);
    }
    s.split(',')
        .map(|v| {
            let v = v.trim();
            if let Some(h) = v.strip_prefix("0x") {
                Ok(i32::from_str_radix(h, 16)?)
            } else {
                Ok(v.parse()?)
            }
        })
        .collect()
}
fn kind(s: &str) -> Result<u16> {
    if let Some(h) = s.strip_prefix("0x") {
        Ok(u16::from_str_radix(h, 16)?)
    } else {
        Ok(s.parse()?)
    }
}
impl Content {
    pub fn load(dir: &Path) -> Result<Self> {
        let data = std::fs::read_to_string(dir.join("objects.xml"))?;
        let doc = roxmltree::Document::parse(&data)?;
        let mut objects = BTreeMap::new();
        for n in doc.root_element().children().filter(|n| n.is_element()) {
            let id = kind(n.attribute("type").context("object has no type")?)?;
            let mut projectiles = BTreeMap::new();
            for p in n.children().filter(|p| p.has_tag_name("Projectile")) {
                let index = match p.attribute("id") {
                    Some(index) => index.parse::<u8>()?,
                    None => 0,
                };
                let fixed = number(p, "Damage", 0.)? as i32;
                let projectile = Projectile {
                    speed: number(p, "Speed", 100.)?,
                    lifetime: number(p, "LifetimeMS", 0.)?,
                    min: number(p, "MinDamage", fixed as f32)? as i32,
                    max: number(p, "MaxDamage", fixed as f32)? as i32,
                    amplitude: number(p, "Amplitude", 0.)?,
                    frequency: number(p, "Frequency", 1.)?,
                    passes_cover: child(p, "PassesCover").is_some(),
                    multi_hit: child(p, "MultiHit").is_some(),
                    armor_piercing: child(p, "ArmorPiercing").is_some(),
                    wavy: child(p, "Wavy").is_some(),
                    parametric: child(p, "Parametric").is_some(),
                    boomerang: child(p, "Boomerang").is_some(),
                    magnitude: number(p, "Magnitude", 3.)?,
                    effects: p
                        .children()
                        .filter(|e| e.has_tag_name("ConditionEffect"))
                        .map(|e| -> Result<_> {
                            let index = crate::condition::projectile_effect(
                                e.text().context("condition name missing")?,
                            )?;
                            let duration = std::time::Duration::try_from_secs_f32(
                                e.attribute("duration")
                                    .context("condition duration missing")?
                                    .parse()?,
                            )?
                            .as_millis()
                            .try_into()?;
                            ensure!(duration > 0, "non-positive projectile condition duration");
                            Ok((index, duration))
                        })
                        .collect::<Result<_>>()?,
                };
                ensure!(
                    projectiles.insert(index, projectile).is_none(),
                    "duplicate projectile {index} on object {id:#x}"
                );
            }
            let o = Object {
                kind: id,
                name: n.attribute("id").context("object has no name")?.into(),
                class: text(n, "Class"),
                hp: number(n, "MaxHitPoints", 0.)? as i32,
                size: number(n, "Size", 100.)? as i32,
                mp: number(n, "MaxMagicPoints", 0.)? as i32,
                attack: number(n, "Attack", 0.)? as i32,
                defense: number(n, "Defense", 0.)? as i32,
                speed: number(n, "Speed", 0.)? as i32,
                dexterity: number(n, "Dexterity", 0.)? as i32,
                vitality: number(n, "HpRegen", 0.)? as i32,
                wisdom: number(n, "MpRegen", 0.)? as i32,
                equipment: list(n, "Equipment")?,
                slots: list(n, "SlotTypes")?,
                slot_type: number(n, "SlotType", 0.)? as i32,
                tier: child(n, "Tier")
                    .map(|_| text(n, "Tier").parse::<u8>())
                    .transpose()?,
                soulbound: child(n, "Soulbound").is_some(),
                bag_type: number(n, "BagType", 0.)? as u8,
                put_normal: child(n, "CanPutNormalObjects").is_some(),
                put_soulbound: child(n, "CanPutSoulboundObjects").is_some(),
                bonuses: n
                    .children()
                    .filter(|x| {
                        x.has_tag_name("ActivateOnEquip") && x.text() == Some("IncrementStat")
                    })
                    .map(|x| -> Result<(u8, i32)> {
                        Ok((
                            x.attribute("stat")
                                .context("equipment stat missing")?
                                .parse()?,
                            x.attribute("amount")
                                .context("equipment amount missing")?
                                .parse()?,
                        ))
                    })
                    .collect::<Result<Vec<_>>>()?,
                unsupported_equipment: n.children().any(|x| {
                    x.has_tag_name("ActivateOnEquip") && x.text() != Some("IncrementStat")
                }),
                projectiles,
                shots: number(n, "NumProjectiles", 1.)? as i32,
                arc: number(n, "ArcGap", 11.25)?,
                rate: number(n, "RateOfFire", 1.)?,
                mana_cost: number(n, "MpCost", 0.)? as i32,
                activate: text(n, "Activate"),
                heal: child(n, "Activate")
                    .and_then(|p| p.attribute("amount"))
                    .map(str::parse)
                    .transpose()?
                    .unwrap_or(0),
                stat_increase: n
                    .children()
                    .find(|p| p.has_tag_name("Activate") && p.text() == Some("IncrementStat"))
                    .map(|p| -> Result<(u8, i32)> {
                        Ok((
                            p.attribute("stat")
                                .context("potion stat missing")?
                                .parse()?,
                            p.attribute("amount")
                                .context("potion amount missing")?
                                .parse()?,
                        ))
                    })
                    .transpose()?,
                unlock_portal: n
                    .children()
                    .find(|p| p.has_tag_name("Activate") && p.text() == Some("UnlockPortal"))
                    .map(|p| -> Result<(String, String)> {
                        Ok((
                            p.attribute("dungeonName")
                                .context("unlock dungeonName missing")?
                                .to_owned(),
                            p.attribute("lockedName")
                                .context("unlock lockedName missing")?
                                .to_owned(),
                        ))
                    })
                    .transpose()?,
                create_object: n
                    .children()
                    .find(|p| p.has_tag_name("Activate") && p.text() == Some("Create"))
                    .map(|p| {
                        p.attribute("id")
                            .context("Create activation id missing")
                            .map(str::to_owned)
                    })
                    .transpose()?,
                dungeon_name: child(n, "DungeonName").map(|_| text(n, "DungeonName")),
                consumable: child(n, "Consumable").is_some(),
                xp_boost: child(n, "XpBoost").is_some(),
                resurrects: child(n, "Resurrects").is_some(),
                textures: [texture(n, "Tex1")?, texture(n, "Tex2")?],
                successor: child(n, "SuccessorId").map(|_| text(n, "SuccessorId")),
                doses: if child(n, "Doses").is_some() {
                    let doses: u8 = text(n, "Doses").parse()?;
                    ensure!(doses > 0, "invalid consumable dose count");
                    Some(doses)
                } else if child(n, "Consumable").is_some() {
                    Some(1)
                } else {
                    None
                },
                consumable_effects: consumables::parse(n)
                    .with_context(|| format!("item activations {id:#x}"))?,
                companion: consumables::companion(n)?,
                heal_range: child(n, "Activate")
                    .and_then(|p| p.attribute("range"))
                    .map(str::parse)
                    .transpose()?
                    .unwrap_or(0.),
                activations: n.children().filter(|p| p.has_tag_name("Activate")).count(),
                abilities: abilities::parse(n).with_context(|| format!("ability {id:#x}"))?,
                cooldown_ms: std::time::Duration::try_from_secs_f32(number(n, "Cooldown", 0.5)?)?
                    .as_millis()
                    .try_into()?,
                enemy: child(n, "Enemy").is_some(),
                stasis_immune: child(n, "StasisImmune").is_some(),
                terrain: match text(n, "Terrain").as_str() {
                    "" => 0,
                    "Mountains" => 1,
                    "HighSand" => 2,
                    "HighPlains" => 3,
                    "HighForest" => 4,
                    "MidSand" => 5,
                    "MidPlains" => 6,
                    "MidForest" => 7,
                    "LowSand" => 8,
                    "LowPlains" => 9,
                    "LowForest" => 10,
                    "ShoreSand" => 11,
                    "ShorePlains" => 12,
                    terrain => bail!("unknown selected terrain {terrain}"),
                },
                quest: child(n, "Quest").is_some(),
                quest_level: child(n, "Level")
                    .map(|_| text(n, "Level").parse())
                    .transpose()?,
                xp_multiplier: number(n, "XpMult", 1.)?,
                blocks: child(n, "OccupySquare").is_some() || child(n, "FullOccupy").is_some(),
                full_occupy: child(n, "FullOccupy").is_some(),
                occupy_square: child(n, "OccupySquare").is_some(),
                enemy_occupy_square: child(n, "EnemyOccupySquare").is_some(),
                static_object: child(n, "Static").is_some(),
                protects_ground: child(n, "ProtectFromGroundDamage").is_some(),
                destructible: child(n, "Enemy").is_some(),
            };
            ensure!(
                objects.insert(id, o).is_none(),
                "duplicate object type {id:#x}"
            );
        }
        let mut classes = BTreeMap::new();
        for n in doc
            .root_element()
            .children()
            .filter(|n| child(*n, "Player").is_some())
        {
            let id = kind(n.attribute("type").context("class type missing")?)?;
            let mut definition = Class {
                base: [0; 8],
                caps: [0; 8],
                growth: [(0, 0); 8],
                requires: vec![],
            };
            for (i, (_, tag)) in STATS.iter().enumerate() {
                let stat = child(n, tag).context("class base stat missing")?;
                definition.base[i] = stat
                    .text()
                    .context("class base stat empty")?
                    .trim()
                    .parse()?;
                definition.caps[i] = stat
                    .attribute("max")
                    .context("class cap missing")?
                    .parse()?;
                let growth = n
                    .children()
                    .find(|p| p.has_tag_name("LevelIncrease") && p.text() == Some(tag))
                    .context("class growth missing")?;
                definition.growth[i] = (
                    growth
                        .attribute("min")
                        .context("growth minimum missing")?
                        .parse()?,
                    growth
                        .attribute("max")
                        .context("growth maximum missing")?
                        .parse()?,
                );
                let (min, max) = definition.growth[i];
                ensure!(
                    definition.base[i] >= 0
                        && definition.caps[i] >= definition.base[i]
                        && min >= 0
                        && max >= min,
                    "invalid class stat {id:#x} {tag}"
                );
            }
            for unlock in n.children().filter(|p| p.has_tag_name("UnlockLevel")) {
                let name = unlock.text().context("unlock class missing")?;
                let parent = objects
                    .values()
                    .find(|o| o.name == name && o.class == "Player")
                    .context("unknown unlock class")?;
                let level: i32 = unlock
                    .attribute("level")
                    .context("unlock level missing")?
                    .parse()?;
                ensure!((1..=20).contains(&level), "invalid unlock level");
                definition.requires.push((parent.kind, level));
            }
            classes.insert(id, definition);
        }
        ensure!(classes.len() == 13, "unexpected player class count");
        let data = std::fs::read_to_string(dir.join("grounds.xml"))?;
        let doc = roxmltree::Document::parse(&data)?;
        let mut grounds = BTreeMap::new();
        let mut pushes = BTreeMap::new();
        let mut hazards = BTreeMap::new();
        for n in doc.root_element().children().filter(|n| n.is_element()) {
            let id = u8::try_from(kind(n.attribute("type").context("ground type missing")?)?)?;
            let speed = if child(n, "NoWalk").is_some() {
                0.
            } else {
                number(n, "Speed", 1.)?
            };
            let push = if child(n, "Push").is_some() {
                let animation =
                    child(n, "Animate").context("pushing ground lacks animation vector")?;
                let mut vector = [0.; 2];
                for (i, axis) in ["dx", "dy"].iter().enumerate() {
                    if let Some(value) = animation.attribute(*axis) {
                        let velocity: f32 = value.parse()?;
                        ensure!(velocity.is_finite(), "non-finite ground push");
                        vector[i] = -velocity / 1000.;
                    }
                }
                (vector[0], vector[1])
            } else {
                (0., 0.)
            };
            let min = number(n, "MinDamage", 0.)? as i32;
            let max = number(n, "MaxDamage", 0.)? as i32;
            ensure!(min >= 0 && max >= min, "invalid ground damage {id:#x}");
            if max > 0 {
                hazards.insert(
                    id,
                    Hazard {
                        name: n.attribute("id").context("hazard name missing")?.into(),
                        min,
                        max,
                    },
                );
            }
            pushes.insert(id, push);
            ensure!(
                grounds.insert(id, speed).is_none(),
                "duplicate ground type {id:#x}"
            );
        }
        ensure!(
            objects.len() == 2201 && grounds.len() == 173,
            "unexpected 123.5.1 content counts"
        );
        let out = Self {
            objects,
            grounds,
            pushes,
            hazards,
            classes,
        };
        let w = out.get(WIZARD)?;
        ensure!(
            out.unlocked(WIZARD, &BTreeMap::new())?,
            "Wizard must be unlocked for a new account"
        );
        ensure!(
            w.hp > 0 && w.equipment.len() == 12 && w.slots.len() == 12,
            "invalid Wizard definition"
        );
        Ok(out)
    }
    pub fn unlocked(&self, class: u16, records: &BTreeMap<u16, i32>) -> Result<bool> {
        let class = self.classes.get(&class).context("unknown player class")?;
        Ok(class
            .requires
            .iter()
            .all(|(kind, level)| records.get(kind).is_some_and(|best| best >= level)))
    }
    pub fn get(&self, k: u16) -> Result<&Object> {
        self.objects
            .get(&k)
            .with_context(|| format!("unknown object type {k:#x}"))
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct Cell {
    pub ground: u8,
    pub object: Option<u16>,
    pub config: String,
    pub terrain: u8,
    pub region: u8,
}
#[derive(Clone, Debug, Serialize)]
pub struct Map {
    pub width: i32,
    pub height: i32,
    pub cells: Vec<Cell>,
    pub spawns: Vec<(f32, f32)>,
}
struct MapReader {
    data: Vec<u8>,
    pos: usize,
}
impl MapReader {
    fn take(&mut self, n: usize) -> Result<&[u8]> {
        ensure!(n <= self.data.len() - self.pos, "truncated map");
        let p = self.pos;
        self.pos += n;
        Ok(&self.data[p..p + n])
    }
    fn short(&mut self) -> Result<i16> {
        Ok(i16::from_le_bytes(self.take(2)?.try_into()?))
    }
    fn int(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into()?))
    }
    fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn string(&mut self) -> Result<String> {
        let mut n = 0usize;
        for shift in (0..35).step_by(7) {
            let b = self.byte()?;
            n |= ((b & 127) as usize) << shift;
            if b & 128 == 0 {
                return Ok(std::str::from_utf8(self.take(n)?)?.to_owned());
            }
        }
        bail!("map string length overflow")
    }
}
impl Map {
    pub fn load(path: &Path, content: &Content) -> Result<Self> {
        let bytes = std::fs::read(path)?;
        Self::decode(&bytes, content).with_context(|| format!("load map {}", path.display()))
    }
    pub(crate) fn decode(bytes: &[u8], content: &Content) -> Result<Self> {
        let map = Self::decode_cells(bytes, content)?;
        ensure!(!map.spawns.is_empty(), "map has no spawn cells");
        Ok(map)
    }
    pub(crate) fn decode_cells(bytes: &[u8], content: &Content) -> Result<Self> {
        let mut data = Vec::new();
        flate2::read::ZlibDecoder::new(bytes)
            .take(16 * 1024 * 1024 + 1)
            .read_to_end(&mut data)?;
        ensure!(
            data.len() <= 16 * 1024 * 1024,
            "decompressed map exceeds 16 MiB"
        );
        let mut r = MapReader { data, pos: 0 };
        let count = r.short()?;
        ensure!(count > 0, "empty map dictionary");
        let mut dict = Vec::new();
        for _ in 0..count {
            let ground = u8::try_from(r.short()?)?;
            ensure!(
                content.grounds.contains_key(&ground),
                "unknown map ground {ground:#x}"
            );
            let name = r.string()?;
            let object = if name.is_empty() {
                None
            } else {
                let mut matches = content.objects.values().filter(|o| o.name == name);
                let object = matches
                    .next()
                    .with_context(|| format!("unknown map object {name}"))?;
                ensure!(matches.next().is_none(), "ambiguous map object {name}");
                Some(object.kind)
            };
            let config = r.string()?;
            let terrain = r.byte()?;
            let region = r.byte()?;
            dict.push(Cell {
                ground,
                object,
                config,
                terrain,
                region,
            });
        }
        let width = r.int()?;
        let height = r.int()?;
        ensure!(
            width > 0 && height > 0 && width <= 2048 && height <= 2048,
            "map dimensions must be within 1..=2048"
        );
        let mut cells = Vec::new();
        let mut spawns = Vec::new();
        for y in 0..height {
            for x in 0..width {
                let i = usize::try_from(r.short()?)?;
                let c = dict.get(i).context("invalid map dictionary index")?.clone();
                if c.region == 1 {
                    spawns.push((x as f32 + 0.5, y as f32 + 0.5));
                }
                cells.push(c);
            }
        }
        ensure!(r.pos == r.data.len(), "trailing map data");
        Ok(Self {
            width,
            height,
            cells,
            spawns,
        })
    }
    pub fn cell(&self, x: f32, y: f32) -> Option<&Cell> {
        if x < 0. || y < 0. || x >= self.width as f32 || y >= self.height as f32 {
            return None;
        }
        self.cells
            .get(y.floor() as usize * self.width as usize + x.floor() as usize)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn map_cell_decoding_preserves_validation() {
        use std::io::Write;
        let mut content = Content::load(Path::new("data")).unwrap();
        let raw_map = |ground: i16, name: &str, width: i32, height: i32, index: i16| {
            let mut raw = Vec::new();
            raw.extend(1i16.to_le_bytes());
            raw.extend(ground.to_le_bytes());
            raw.push(name.len() as u8);
            raw.extend(name.as_bytes());
            raw.extend([0, 11, 1]);
            raw.extend(width.to_le_bytes());
            raw.extend(height.to_le_bytes());
            raw.extend(index.to_le_bytes());
            raw
        };
        let valid = raw_map(0xbe, "", 1, 1, 0);
        let mut truncated = valid.clone();
        truncated.pop();
        let mut trailing = valid.clone();
        trailing.push(0);
        let mut invalid_utf8 = raw_map(0xbe, "x", 1, 1, 0);
        invalid_utf8[5] = 0xff;
        let mut ambiguous = content.objects[&0x105].clone();
        ambiguous.kind = 0xffff;
        content.objects.insert(ambiguous.kind, ambiguous);
        for (raw, expected) in [
            (vec![], "truncated map"),
            (vec![0, 0], "empty map dictionary"),
            (raw_map(256, "", 1, 1, 0), "out of range"),
            (raw_map(0x08, "", 1, 1, 0), "unknown map ground"),
            (
                raw_map(0xbe, "missing-object", 1, 1, 0),
                "unknown map object",
            ),
            (raw_map(0xbe, "Bush", 1, 1, 0), "ambiguous map object"),
            (invalid_utf8, "invalid utf-8"),
            (
                vec![1, 0, 0xbe, 0, 0x80, 0x80, 0x80, 0x80, 0x80],
                "map string length overflow",
            ),
            (raw_map(0xbe, "", 0, 1, 0), "map dimensions"),
            (raw_map(0xbe, "", 1, -1, 0), "map dimensions"),
            (raw_map(0xbe, "", 2049, 1, 0), "map dimensions"),
            (raw_map(0xbe, "", 1, 2049, 0), "map dimensions"),
            (raw_map(0xbe, "", 1, 1, 1), "invalid map dictionary index"),
            (raw_map(0xbe, "", 1, 1, -1), "out of range"),
            (truncated, "truncated map"),
            (trailing, "trailing map data"),
            (
                vec![0; 16 * 1024 * 1024 + 1],
                "decompressed map exceeds 16 MiB",
            ),
        ] {
            let mut encoder =
                flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
            encoder.write_all(&raw).unwrap();
            let bytes = encoder.finish().unwrap();
            for decode in [Map::decode, Map::decode_cells] {
                let error = format!("{:#}", decode(&bytes, &content).unwrap_err());
                assert!(error.contains(expected), "expected {expected}: {error}");
            }
            let source = crate::terrain::realms::Source::encoded_fixture("malformed-test", bytes);
            let error = format!("{:#}", source.open(&content).unwrap_err());
            assert!(
                error.contains("decode Realm layout malformed-test"),
                "{error}"
            );
            assert!(error.contains(expected), "expected {expected}: {error}");
        }
        for bytes in [&b"not zlib"[..], &b"\x78\x9c\xff\xff\xff\xff"[..]] {
            assert!(Map::decode_cells(bytes, &content).is_err());
            assert!(Map::decode(bytes, &content).is_err());
        }
    }

    #[test]
    fn selected_legacy_sounds_are_present() {
        let xml = std::fs::read_to_string("data/objects.xml").unwrap();
        let doc = roxmltree::Document::parse(&xml).unwrap();
        let sounds: std::collections::BTreeSet<_> = doc
            .descendants()
            .filter(|node| node.has_tag_name("OldSound"))
            .filter_map(|node| node.text())
            .collect();
        assert!(!sounds.is_empty());
        for sound in sounds {
            let path = Path::new("data/sfx").join(format!("{sound}.mp3"));
            let bytes = std::fs::read(&path).unwrap_or_else(|error| {
                panic!("missing selected sound {}: {error}", path.display())
            });
            assert!(!bytes.is_empty(), "empty selected sound {}", path.display());
        }
    }

    #[test]
    fn client_data_identity() {
        let c = Content::load(Path::new("data")).unwrap();
        let w = c.get(WIZARD).unwrap();
        assert_eq!(w.equipment[0], 0xa97);
        assert_eq!(w.equipment[1], 0xa2e);
        assert_eq!(w.hp, 100);
        assert!(c.grounds.contains_key(&0x61) && c.grounds.contains_key(&0x65));
        assert!(c.objects[&0x120].static_object);
        assert!(!c.objects[&0x683].static_object);
        assert!(c.objects[&0x120].occupy_square && c.objects[&0x120].enemy_occupy_square);
        assert!(c.objects[&0x100].occupy_square && !c.objects[&0x100].enemy_occupy_square);
        assert!(!c.objects[&0x683].projectiles[&0].passes_cover);
        assert!(c.objects[&0xa08].projectiles[&0].passes_cover);
    }
    #[test]
    fn projectile_identity_includes_both_selected_dreadstump_attacks() {
        let content = Content::load(Path::new("data")).unwrap();
        let boss = &content.objects[&0x927];
        assert_eq!(boss.projectiles.len(), 2);
        let slow = &boss.projectiles[&0];
        assert_eq!(
            (slow.min, slow.max, slow.speed, slow.lifetime),
            (17, 17, 75., 1500.)
        );
        let fast = &boss.projectiles[&1];
        assert_eq!(
            (fast.min, fast.max, fast.speed, fast.lifetime),
            (25, 25, 130., 1100.)
        );
        assert_eq!(content.objects[&0xa97].projectiles.len(), 1);
        assert!(content.objects[&0xa97].projectiles.contains_key(&0));
    }
    #[test]
    fn class_growth_caps_and_unlocks_are_selected_xml() {
        let content = Content::load(Path::new("data")).unwrap();
        assert_eq!(content.classes.len(), 13);
        let wizard = &content.classes[&WIZARD];
        assert_eq!(wizard.base, [100, 100, 12, 0, 10, 15, 12, 12]);
        assert_eq!(wizard.caps, [670, 385, 75, 25, 50, 75, 40, 60]);
        assert_eq!(
            wizard.growth,
            [
                (20, 30),
                (5, 15),
                (1, 2),
                (0, 0),
                (0, 2),
                (1, 2),
                (0, 1),
                (0, 2)
            ]
        );
        assert!(wizard.requires.is_empty());
        assert_eq!(content.classes[&0x0310].requires, vec![(WIZARD, 5)]);
        assert_eq!(
            content.classes[&0x031f].requires,
            vec![(0x031e, 20), (0x0310, 20)]
        );
    }
    #[test]
    fn sourced_world_maps_resolve_and_cave_spawn_reaches_boss() {
        let content = Content::load(Path::new("data")).unwrap();
        for name in [
            "nexus",
            "nexus-limbo",
            "vault",
            "realm-shore",
            "pirate-cave",
        ] {
            let map = Map::load(&Path::new("data").join(format!("{name}.wmap")), &content).unwrap();
            assert!(!map.spawns.is_empty());
            assert_eq!(map.cells.len(), (map.width * map.height) as usize);
            if name == "realm-shore" {
                assert_eq!((map.width, map.height), (96, 96));
                assert_eq!(map.cells.iter().filter(|c| c.terrain == 11).count(), 3460);
                assert_eq!(map.cells.iter().filter(|c| c.terrain == 12).count(), 3200);
            }
        }
        let cave = Map::load(Path::new("data/pirate-cave.wmap"), &content).unwrap();
        assert_eq!((cave.width, cave.height), (94, 98));
        assert_eq!(cave.spawns, vec![(20.5, 82.5)]);
        let boss = cave
            .cells
            .iter()
            .position(|c| c.object == Some(0x927))
            .unwrap();
        let spawn = 82 * cave.width as usize + 20;
        let mut reached = std::collections::BTreeSet::from([spawn]);
        let mut queue = std::collections::VecDeque::from([spawn]);
        while let Some(i) = queue.pop_front() {
            let (x, y) = (i as i32 % cave.width, i as i32 / cave.width);
            for (x, y) in [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)] {
                if x < 0 || y < 0 || x >= cave.width || y >= cave.height {
                    continue;
                }
                let next = (y * cave.width + x) as usize;
                let cell = &cave.cells[next];
                if content.grounds[&cell.ground] > 0.
                    && cell
                        .object
                        .is_none_or(|kind| !content.objects[&kind].blocks)
                    && reached.insert(next)
                {
                    queue.push_back(next);
                }
            }
        }
        assert!(
            reached.contains(&boss),
            "sourced cave boss is disconnected from spawn"
        );
        assert!(cave.cells.iter().any(|c| c.config.starts_with("conn:")));
    }
    #[test]
    fn selected_priest_starter_and_tiered_healing_are_exact() {
        let c = Content::load(Path::new("data")).unwrap();
        assert_eq!(c.get(PRIEST).unwrap().slots[..4], [8, 4, 14, 9]);
        assert_eq!(
            c.get(PRIEST).unwrap().equipment[..5],
            [0xa04, 0xa31, -1, -1, 0xa22]
        );
        for (tier, kind, amount, range, cost) in [
            (0, 0xa31, 100, 5., 60),
            (1, 0xad7, 130, 5.5, 65),
            (2, 0xa32, 165, 6., 70),
            (3, 0xad8, 205, 6.5, 75),
            (4, 0xa33, 250, 7., 80),
            (5, 0xa5b, 300, 8., 85),
            (6, 0xb25, 300, 8., 85),
        ] {
            let o = c.get(kind).unwrap();
            assert_eq!(
                (o.tier, o.heal, o.heal_range, o.mana_cost),
                (Some(tier), amount, range, cost)
            );
            assert_eq!(
                (o.activate.as_str(), o.activations, o.cooldown_ms),
                ("HealNova", 1, 500)
            );
        }
    }
    #[test]
    fn candidate_map_is_lossless() {
        let c = Content::load(Path::new("data")).unwrap();
        let m = Map::load(Path::new("data/tutorial.wmap"), &c).unwrap();
        assert_eq!((m.width, m.height, m.cells.len()), (25, 201, 5025));
        assert_eq!(m.spawns.len(), 12);
        assert!(m.cells.iter().any(|c| c.object == Some(0x071e)));
    }
}
