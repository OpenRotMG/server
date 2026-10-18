use crate::content::{Cell, Content, Map};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

const SIDE: usize = 2048;
const CELLS: usize = SIDE * SIDE;
const OBJECTS: &str = "90ab4dbddadfc5adfa77f87746c376d6c611b7b3e02ec0ed8ab4f0ad91534b72";
const GROUNDS: &str = "436b596b062c141112790bb3702e4b25e639a5a27541f2a591c462cf9e1b15f7";

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Recipe {
    schema: u8,
    pub(super) id: String,
    pub(super) profile: Profile,
    width: usize,
    height: usize,
    orientation: Orientation,
    scenery_seed: u64,
    content: ContentIdentity,
    layers: Layers,
    pub(super) output: String,
    provenance: Provenance,
}
#[derive(Debug, Deserialize, Serialize)]
pub(super) enum Profile {
    #[serde(rename = "classic-v1")]
    ClassicV1,
}
impl Profile {
    pub(super) fn name(&self) -> &'static str {
        match self {
            Self::ClassicV1 => "classic-v1",
        }
    }
}
#[derive(Debug, Deserialize, Serialize)]
enum Orientation {
    #[serde(rename = "x-major")]
    XMajor,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ContentIdentity {
    objects: String,
    grounds: String,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Layers {
    elevation: Layer,
    moisture: Layer,
    overrides: Layer,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Layer {
    path: String,
    sha256: String,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Provenance {
    source_url: String,
    source_sha256: String,
    exporter_url: String,
    exporter_sha256: String,
    emulator: String,
    mode: String,
    seed_variant: String,
    status: String,
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub(super) fn valid_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub(super) fn safe_path(root: &Path, relative: &str) -> Result<PathBuf> {
    ensure!(!relative.is_empty(), "empty recipe path");
    let mut path = root.to_path_buf();
    for part in Path::new(relative).components() {
        let Component::Normal(name) = part else {
            bail!("unsafe recipe path {relative}");
        };
        path.push(name);
        match path.symlink_metadata() {
            Ok(meta) => ensure!(
                !meta.file_type().is_symlink(),
                "symlink recipe path {relative}"
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e).with_context(|| format!("inspect {}", path.display())),
        }
    }
    Ok(path)
}
pub(super) fn bounded_read(path: &Path, max: usize) -> Result<Vec<u8>> {
    let file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    ensure!(
        file.metadata()?.is_file(),
        "not a regular file: {}",
        path.display()
    );
    let mut bytes = Vec::new();
    file.take(max as u64 + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= max,
        "file exceeds {max} bytes: {}",
        path.display()
    );
    Ok(bytes)
}
pub(super) fn valid_id(id: &str) -> bool {
    id.split('-').all(|part| {
        !part.is_empty()
            && part
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    })
}
impl Recipe {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.schema == 1,
            "unsupported recipe schema {}",
            self.schema
        );
        ensure!(
            valid_id(&self.id),
            "invalid Realm ID {}: expected lowercase ASCII alphanumeric segments separated by hyphens",
            self.id
        );
        ensure!(
            self.width == SIDE && self.height == SIDE,
            "expected 2048-square layers"
        );
        ensure!(
            self.content.objects == OBJECTS && self.content.grounds == GROUNDS,
            "unsupported content identity"
        );
        ensure!(
            self.output == format!("{}.wmap", self.id),
            "output must match Realm ID"
        );
        let p = &self.provenance;
        ensure!(
            valid_hash(&p.source_sha256) && valid_hash(&p.exporter_sha256),
            "invalid provenance hash"
        );
        ensure!(
            p.source_url.starts_with("https://")
                && p.exporter_url.starts_with("https://")
                && !p.emulator.is_empty()
                && p.mode == "Perlin"
                && !p.seed_variant.is_empty()
                && !p.status.is_empty(),
            "incomplete export provenance"
        );
        Ok(())
    }
}
fn recipes(root: &Path) -> Result<BTreeMap<String, Recipe>> {
    let mut paths = std::fs::read_dir(root)
        .with_context(|| format!("discover recipes in {}", root.display()))?
        .map(|entry| Ok(entry?.path()))
        .collect::<Result<Vec<_>>>()?;
    paths.sort();
    let mut found = BTreeMap::new();
    for path in paths
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
    {
        let name = path
            .file_name()
            .context("recipe filename missing")?
            .to_str()
            .context("non-UTF8 recipe path")?;
        let recipe = read_recipe(root, name)?;
        ensure!(
            found.insert(recipe.id.clone(), recipe).is_none(),
            "duplicate Realm recipe ID"
        );
    }
    Ok(found)
}
pub(super) fn read_recipe(root: &Path, relative: &str) -> Result<Recipe> {
    let path = safe_path(root, relative)?;
    let recipe: Recipe = serde_json::from_slice(&bounded_read(&path, 64 * 1024)?)
        .with_context(|| format!("parse recipe {}", path.display()))?;
    recipe
        .validate()
        .with_context(|| format!("validate recipe {}", path.display()))?;
    for relative in [
        &recipe.layers.elevation.path,
        &recipe.layers.moisture.path,
        &recipe.layers.overrides.path,
        &recipe.output,
    ] {
        safe_path(root, relative).with_context(|| format!("recipe {}", path.display()))?;
    }
    Ok(recipe)
}
fn expand_layer(bytes: &[u8], count: usize, expected: &str) -> Result<Vec<u8>> {
    ensure!(
        count <= CELLS && valid_hash(expected),
        "invalid layer size/hash"
    );
    ensure!(
        bytes.len() <= CELLS + 4096,
        "compressed layer exceeds bound"
    );
    let mut decoder = flate2::Decompress::new(true);
    let mut raw = Vec::with_capacity(count + 1);
    let status = decoder
        .decompress_vec(bytes, &mut raw, flate2::FlushDecompress::Finish)
        .context("inflate layer")?;
    ensure!(
        status == flate2::Status::StreamEnd,
        "incomplete or oversized layer stream"
    );
    ensure!(
        raw.len() == count,
        "layer length: expected {count}, got {}",
        raw.len()
    );
    ensure!(
        decoder.total_in() as usize == bytes.len(),
        "trailing compressed layer data"
    );
    let actual = sha256(&raw);
    ensure!(
        actual == expected,
        "layer checksum: expected {expected}, got {actual}"
    );
    Ok(raw)
}
fn load_layer(root: &Path, layer: &Layer) -> Result<Vec<u8>> {
    let path = safe_path(root, &layer.path)?;
    let bytes = bounded_read(&path, CELLS + 4096)?;
    expand_layer(&bytes, CELLS, &layer.sha256).with_context(|| format!("layer {}", path.display()))
}
fn transpose(raw: &[u8], width: usize, height: usize) -> Result<Vec<u8>> {
    ensure!(
        width > 0 && height > 0 && width <= SIDE && height <= SIDE && raw.len() == width * height,
        "invalid x-major dimensions/length"
    );
    Ok((0..height)
        .flat_map(|y| (0..width).map(move |x| raw[x * height + y]))
        .collect())
}

fn habitat(elevation: u8, moisture: u8) -> (u8, u8) {
    let wet = moisture as u16 * 7;
    if elevation >= 204 {
        (if wet > 5 * 255 { 0x6a } else { 0x60 }, 1)
    } else if elevation > 153 {
        if wet > 5 * 255 {
            (0x47, 4)
        } else if wet > 3 * 255 {
            (0x23, 3)
        } else {
            (0xbd, 2)
        }
    } else if elevation > 51 {
        if wet > 5 * 255 {
            (0x56, 7)
        } else if wet > 3 * 255 {
            (0x46, 6)
        } else {
            (0xbd, 5)
        }
    } else if wet > 5 * 255 {
        (0x57, 10)
    } else if wet > 4 * 255 {
        (0x56, 10)
    } else if wet > 3 * 255 {
        (0x48, 9)
    } else {
        (0xbd, 8)
    }
}
fn cell(elevation: u8, moisture: u8, override_byte: u8) -> Result<Cell> {
    ensure!(
        matches!(override_byte & 15, 0 | 1 | 3),
        "unknown polygon flags {override_byte:#x}"
    );
    let (ground, terrain) = match override_byte >> 4 {
        0 => habitat(elevation, moisture),
        // The generic demo's lava and ice are not authority for new gameplay hazards.
        1 | 2 => (0x73, 0),
        3 => (0x6a, 1),
        4 | 6 => (0xbc, 0),
        5 => (0xbc, 0),
        7 => (0x72, 0),
        8 => (0xbe, 11),
        9..=12 => (0xd0, 0),
        _ => bail!("unknown override {override_byte:#x}"),
    };
    Ok(Cell {
        ground,
        object: None,
        config: String::new(),
        terrain,
        region: 0,
    })
}
#[derive(Debug, Serialize)]
struct Mainland {
    removed_components: usize,
    removed_cells: usize,
}
fn neighbors(i: usize, width: usize) -> [usize; 4] {
    let x = i % width;
    [
        i.wrapping_sub(width),
        i + width,
        if x > 0 { i - 1 } else { usize::MAX },
        if x + 1 < width { i + 1 } else { usize::MAX },
    ]
}
fn normalize_mainland(cells: &mut [Cell], overrides: &[u8], width: usize) -> Result<Mainland> {
    let mut labels = vec![0usize; cells.len()];
    let mut counts = Vec::new();
    let mut queue = VecDeque::new();
    // Original ocean codes distinguish lakes from ocean even when their final grounds are identical.
    for start in 0..cells.len() {
        if labels[start] != 0 || overrides[start] >> 4 == 5 {
            continue;
        }
        let label = counts.len() + 1;
        let mut count = 0;
        labels[start] = label;
        queue.push_back(start);
        while let Some(i) = queue.pop_front() {
            count += 1;
            for next in neighbors(i, width) {
                if next < cells.len() && labels[next] == 0 && overrides[next] >> 4 != 5 {
                    labels[next] = label;
                    queue.push_back(next);
                }
            }
        }
        counts.push(count);
    }
    let largest = *counts.iter().max().context("no non-ocean mainland")?;
    ensure!(
        counts.iter().filter(|&&n| n == largest).count() == 1,
        "ambiguous largest mainland"
    );
    let mainland = counts
        .iter()
        .position(|&n| n == largest)
        .context("mainland label missing")?
        + 1;
    let mut removed_cells = 0;
    for (i, c) in cells.iter_mut().enumerate() {
        if labels[i] != 0 && labels[i] != mainland {
            *c = cell(0, 0, 0x50)?;
            removed_cells += 1;
        }
    }
    Ok(Mainland {
        removed_components: counts.len() - 1,
        removed_cells,
    })
}
fn assemble(
    e: &[u8],
    m: &[u8],
    o: &[u8],
    width: usize,
    height: usize,
    seed: u64,
) -> Result<(Map, Mainland)> {
    let e = transpose(e, width, height)?;
    let m = transpose(m, width, height)?;
    let o = transpose(o, width, height)?;
    let mut cells = (0..e.len())
        .map(|i| {
            cell(e[i], m[i], o[i]).with_context(|| format!("cell {},{}", i % width, i / width))
        })
        .collect::<Result<Vec<_>>>()?;
    let mainland = normalize_mainland(&mut cells, &o, width)?;
    let mut coast_distance = vec![u8::MAX; cells.len()];
    let mut queue = VecDeque::new();
    for (i, _) in cells.iter().enumerate().filter(|(_, c)| c.terrain == 11) {
        coast_distance[i] = 0;
        queue.push_back(i);
    }
    while let Some(i) = queue.pop_front() {
        let distance = coast_distance[i] + 1;
        if distance > 8 {
            continue;
        }
        for next in neighbors(i, width) {
            if next < cells.len() && coast_distance[next] > distance {
                coast_distance[next] = distance;
                queue.push_back(next);
            }
        }
    }
    for (i, c) in cells.iter_mut().enumerate() {
        if coast_distance[i] <= 8 && matches!(c.terrain, 9 | 10) && o[i] >> 4 == 0 {
            c.terrain = 12;
        }
        super::decorate(c, super::mix(seed ^ 0xbb67ae8584caa73b ^ i as u64))?;
    }
    Ok((
        Map {
            width: width as i32,
            height: height as i32,
            cells,
            spawns: vec![],
        },
        mainland,
    ))
}

type Entry = (u8, String, String, u8, u8);
fn encode(map: &Map, content: &Content) -> Result<(Vec<u8>, usize)> {
    ensure!(
        map.width > 0
            && map.height > 0
            && map.width <= SIDE as i32
            && map.height <= SIDE as i32
            && map.cells.len() == map.width as usize * map.height as usize,
        "invalid output dimensions/cells"
    );
    let mut entries = BTreeSet::new();
    let mut validated_objects = BTreeSet::new();
    for c in &map.cells {
        ensure!(
            content.grounds.contains_key(&c.ground) && !content.hazards.contains_key(&c.ground),
            "unknown/hazardous output ground {:#x}",
            c.ground
        );
        ensure!(
            c.terrain <= 12 && c.region == 0,
            "unsupported output terrain/region"
        );
        let name = match c.object {
            Some(kind) => {
                let object = content.get(kind)?;
                ensure!(
                    object.static_object && !object.enemy,
                    "non-scenery output object {kind:#x}"
                );
                if validated_objects.insert(kind) {
                    ensure!(
                        content
                            .objects
                            .values()
                            .filter(|o| o.name == object.name)
                            .count()
                            == 1,
                        "ambiguous output object {}",
                        object.name
                    );
                }
                ensure!(!object.name.is_empty(), "empty output object name");
                if !c.config.is_empty() {
                    let size: u16 = c
                        .config
                        .strip_prefix("size:")
                        .context("unsupported scenery configuration")?
                        .parse()?;
                    ensure!(
                        matches!(kind, 0x100..=0x104 | 0x185..=0x18d | 0x197..=0x19f)
                            && (130..=160).contains(&size)
                            && size.is_multiple_of(5)
                            && c.config == format!("size:{size}"),
                        "unsupported scenery size/configuration"
                    );
                }
                object.name.clone()
            }
            None => {
                ensure!(c.config.is_empty(), "configuration without scenery");
                String::new()
            }
        };
        entries.insert((c.ground, name, c.config.clone(), c.terrain, c.region));
    }
    ensure!(
        !entries.is_empty() && entries.len() <= i16::MAX as usize,
        "output dictionary exceeds format limit"
    );
    let dict: BTreeMap<Entry, i16> = entries
        .into_iter()
        .enumerate()
        .map(|(i, e)| (e, i as i16))
        .collect();
    let mut raw = Vec::new();
    raw.extend((dict.len() as i16).to_le_bytes());
    for (ground, name, config, terrain, region) in dict.keys() {
        raw.extend((*ground as i16).to_le_bytes());
        for text in [name, config] {
            ensure!(text.len() <= 1024, "output dictionary string exceeds limit");
            let mut length = text.len();
            while length >= 128 {
                raw.push((length as u8 & 127) | 128);
                length >>= 7;
            }
            raw.push(length as u8);
            raw.extend(text.as_bytes());
        }
        raw.extend([*terrain, *region]);
    }
    raw.extend(map.width.to_le_bytes());
    raw.extend(map.height.to_le_bytes());
    for c in &map.cells {
        let name = match c.object {
            Some(kind) => content.get(kind)?.name.clone(),
            None => String::new(),
        };
        raw.extend(dict[&(c.ground, name, c.config.clone(), c.terrain, c.region)].to_le_bytes());
    }
    ensure!(
        raw.len() <= 16 * 1024 * 1024,
        "output exceeds expanded WMAP limit"
    );
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    encoder.write_all(&raw)?;
    Ok((encoder.finish()?, dict.len()))
}

pub(crate) fn run(args: &[String]) -> Result<bool> {
    if args.first().is_none_or(|a| a != "build-realm") {
        return Ok(false);
    }
    let mut data = None;
    let mut id = None;
    let mut check = false;
    let mut args = args[1..].iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--data" if data.is_none() => {
                data = Some(PathBuf::from(args.next().context("--data requires DIR")?))
            }
            "--id" if id.is_none() => id = Some(args.next().context("--id requires ID")?.as_str()),
            "--check" if !check => check = true,
            _ => bail!("unknown/duplicate build-realm argument {arg}"),
        }
    }
    let data = data.context("build-realm requires --data DIR")?;
    let id = id.context("build-realm requires --id ID")?;
    let started = std::time::Instant::now();
    let root = safe_path(&data, "realms")?;
    let recipes = recipes(&root)?;
    let recipe = recipes
        .get(id)
        .with_context(|| format!("unknown Realm recipe ID {id}"))?;
    for (name, expected) in [("objects.xml", OBJECTS), ("grounds.xml", GROUNDS)] {
        let actual = sha256(&bounded_read(&safe_path(&data, name)?, 16 * 1024 * 1024)?);
        ensure!(
            actual == expected,
            "unsupported content {name}: expected {expected}, got {actual}"
        );
    }
    let content = Content::load(&data).context("load selected-client content")?;
    let layers = &recipe.layers;
    let e = load_layer(&root, &layers.elevation)?;
    let m = load_layer(&root, &layers.moisture)?;
    let o = load_layer(&root, &layers.overrides)?;
    let (map, mainland) = match (&recipe.profile, &recipe.orientation) {
        (Profile::ClassicV1, Orientation::XMajor) => {
            assemble(&e, &m, &o, recipe.width, recipe.height, recipe.scenery_seed)?
        }
    };
    let prepared = started.elapsed();
    let (bytes, dictionary) = encode(&map, &content)?;
    let output = safe_path(&root, &recipe.output)?;
    if check {
        ensure!(
            bounded_read(&output, 16 * 1024 * 1024)? == bytes,
            "rebuilt output differs: {}",
            output.display()
        );
    } else {
        std::fs::write(&output, &bytes).with_context(|| format!("write {}", output.display()))?;
    }
    let mut habitats = BTreeMap::<u8, usize>::new();
    for cell in &map.cells {
        *habitats.entry(cell.terrain).or_default() += 1;
    }
    println!(
        "{}",
        serde_json::json!({
            "id":id, "profile":"classic-v1", "recipe_identity_sha256":sha256(&serde_json::to_vec(recipe)?),
            "status":if check {"checked"} else {"built"}, "output":output,
            "sha256":sha256(&bytes), "bytes":bytes.len(), "width":map.width, "height":map.height,
            "dictionary":dictionary, "habitats":habitats, "mainland":mainland,
            "scenery":map.cells.iter().filter(|c| c.object.is_some()).count(),
            "configured_scenery":map.cells.iter().filter(|c| !c.config.is_empty()).count(),
            "roads":map.cells.iter().filter(|c| c.ground == 0xd0).count(),
            "water":map.cells.iter().filter(|c| matches!(c.ground, 0xbc | 0x72 | 0x73)).count(),
            "prepare_ms":prepared.as_millis(), "total_ms":started.elapsed().as_millis(),
            "reconstruction":recipe.provenance.status,
        })
    );
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zipped(raw: &[u8]) -> Vec<u8> {
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
        encoder.write_all(raw).unwrap();
        encoder.finish().unwrap()
    }
    fn signature(map: &Map) -> Vec<(u8, Option<u16>, String, u8, u8)> {
        map.cells
            .iter()
            .map(|c| (c.ground, c.object, c.config.clone(), c.terrain, c.region))
            .collect()
    }
    fn recipe_value() -> serde_json::Value {
        serde_json::from_str(include_str!("../../data/realms/world02.json")).unwrap()
    }
    fn recipe(value: serde_json::Value) -> Result<Recipe> {
        let recipe: Recipe = serde_json::from_value(value)?;
        recipe.validate()?;
        Ok(recipe)
    }

    #[test]
    fn realm_prepare_asymmetric_transpose_and_full_cells() {
        assert_eq!(
            transpose(&[0, 1, 2, 3, 4, 5], 3, 2).unwrap(),
            [0, 2, 4, 1, 3, 5]
        );
        assert_eq!(
            transpose(&[0, 1, 2, 3, 4, 5], 2, 3).unwrap(),
            [0, 3, 1, 4, 2, 5]
        );
        let (map, _) = assemble(
            &[0; 6],
            &[0; 6],
            &[0x50, 0x80, 0x10, 0x30, 0x60, 0x90],
            3,
            2,
            17,
        )
        .unwrap();
        assert_eq!(
            signature(&map),
            [0xbc, 0x73, 0xbc, 0xbe, 0x6a, 0xd0]
                .into_iter()
                .zip([0, 0, 0, 11, 1, 0])
                .map(|(g, t)| (g, None, String::new(), t, 0))
                .collect::<Vec<_>>()
        );
        assert!(map.spawns.is_empty());
        for (raw, w, h) in [
            (&[0][..], 0, 1),
            (&[0][..], 1, 0),
            (&[0][..], 2, 1),
            (&[][..], SIDE + 1, 1),
        ] {
            assert!(transpose(raw, w, h).is_err());
        }
    }

    #[test]
    fn realm_prepare_threshold_boundaries_and_selected_habitats() {
        for (e, m, expected) in [
            (0, 109, (0xbd, 8)),
            (0, 110, (0x48, 9)),
            (0, 145, (0x48, 9)),
            (0, 146, (0x56, 10)),
            (0, 182, (0x56, 10)),
            (0, 183, (0x57, 10)),
            (51, 0, (0xbd, 8)),
            (52, 0, (0xbd, 5)),
            (153, 110, (0x46, 6)),
            (154, 110, (0x23, 3)),
            (203, 183, (0x47, 4)),
            (204, 183, (0x6a, 1)),
            (204, 182, (0x60, 1)),
            (255, 0, (0x60, 1)),
        ] {
            assert_eq!(habitat(e, m), expected, "e={e} m={m}");
        }
        let mut terrains = BTreeSet::new();
        for e in 0..=255u8 {
            for m in 0..=255u8 {
                let (g, t) = habitat(e, m);
                let expected_t = if e >= 204 {
                    1
                } else if e > 153 {
                    if m >= 183 {
                        4
                    } else if m >= 110 {
                        3
                    } else {
                        2
                    }
                } else if e > 51 {
                    if m >= 183 {
                        7
                    } else if m >= 110 {
                        6
                    } else {
                        5
                    }
                } else if m >= 146 {
                    10
                } else if m >= 110 {
                    9
                } else {
                    8
                };
                assert_eq!(t, expected_t);
                assert!(matches!(
                    g,
                    0xbd | 0x48 | 0x56 | 0x57 | 0x46 | 0x23 | 0x47 | 0x60 | 0x6a
                ));
                terrains.insert(t);
            }
        }
        assert_eq!(terrains, (1..=10).collect());
    }

    #[test]
    fn realm_prepare_known_unknown_overrides_and_no_hazards() {
        let content = Content::load(Path::new("data")).unwrap();
        let pairs = [
            (0xbd, 8),
            (0x73, 0),
            (0x73, 0),
            (0x6a, 1),
            (0xbc, 0),
            (0xbc, 0),
            (0xbc, 0),
            (0x72, 0),
            (0xbe, 11),
            (0xd0, 0),
            (0xd0, 0),
            (0xd0, 0),
            (0xd0, 0),
        ];
        for (kind, expected) in pairs.into_iter().enumerate() {
            for flags in [0, 1, 3] {
                let c = cell(0, 0, (kind as u8) << 4 | flags).unwrap();
                assert_eq!((c.ground, c.terrain), expected);
                assert!(c.object.is_none() && c.config.is_empty() && c.region == 0);
                assert!(!content.hazards.contains_key(&c.ground));
            }
        }
        for byte in 0..=255u8 {
            assert_eq!(
                cell(0, 0, byte).is_ok(),
                byte >> 4 <= 12 && matches!(byte & 15, 0 | 1 | 3),
                "override {byte:#x}"
            );
        }
    }

    #[test]
    fn realm_prepare_coast_distance_fixture() {
        let mut overrides = [0; 12];
        overrides[0] = 0x80;
        let (map, _) = assemble(&[0; 12], &[200; 12], &overrides, 12, 1, 10).unwrap();
        assert_eq!(
            map.cells
                .iter()
                .map(|c| (c.ground, c.terrain))
                .collect::<Vec<_>>(),
            [(0xbe, 11)]
                .into_iter()
                .chain(std::iter::repeat_n((0x57, 12), 8))
                .chain(std::iter::repeat_n((0x57, 10), 3))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn realm_prepare_checksums_lengths_and_compression_bounds() {
        let raw = [1, 2, 3, 4, 5, 6];
        let compressed = zipped(&raw);
        let hash = sha256(&raw);
        assert_eq!(expand_layer(&compressed, 6, &hash).unwrap(), raw);
        for count in [0, 5, 7, CELLS + 1] {
            assert!(expand_layer(&compressed, count, &hash).is_err());
        }
        for hash in ["x".to_string(), "0".repeat(64), hash.to_uppercase()] {
            assert!(expand_layer(&compressed, 6, &hash).is_err());
        }
        for remove in 1..compressed.len() {
            assert!(
                expand_layer(&compressed[..compressed.len() - remove], 6, &hash).is_err(),
                "removed {remove} bytes"
            );
        }
        let mut trailing = compressed.clone();
        trailing.push(0);
        assert!(expand_layer(&trailing, 6, &hash).is_err());
        let mut corrupt = compressed;
        corrupt[4] ^= 0xff;
        assert!(expand_layer(&corrupt, 6, &hash).is_err());
        assert!(expand_layer(&vec![0; CELLS + 4097], 6, &hash).is_err());
        assert!(expand_layer(&zipped(&vec![0; 10000]), 6, &hash).is_err());
        assert!(
            load_layer(
                Path::new("data/realms"),
                &Layer {
                    path: "absent.zlib".into(),
                    sha256: hash
                }
            )
            .is_err()
        );
    }

    #[test]
    fn realm_prepare_strict_recipes_paths_and_cli() {
        recipe(recipe_value()).unwrap();
        for (field, value) in [
            ("unknown", serde_json::json!(0)),
            ("schema", serde_json::json!(2)),
            ("id", serde_json::json!("../world2")),
            ("profile", serde_json::json!("private-curve")),
            ("orientation", serde_json::json!("row-major")),
            ("width", serde_json::json!(4096)),
            ("output", serde_json::json!("../world2.wmap")),
        ] {
            let mut v = recipe_value();
            v[field] = value;
            assert!(recipe(v).is_err(), "{field}");
        }
        for path in [
            vec!["layers", "elevation"],
            vec!["layers"],
            vec!["content"],
            vec!["provenance"],
        ] {
            let mut v = recipe_value();
            let mut target = &mut v;
            for part in path {
                target = &mut target[part];
            }
            target["unknown"] = serde_json::json!(0);
            assert!(recipe(v).is_err());
        }
        let mut v = recipe_value();
        v["content"]["objects"] = serde_json::json!("0".repeat(64));
        assert!(recipe(v).is_err());
        let mut v = recipe_value();
        v.as_object_mut().unwrap().remove("provenance");
        assert!(recipe(v).is_err());
        for path in [
            "",
            "../outside",
            "/absolute",
            "x/../../outside",
            "./relative",
        ] {
            assert!(safe_path(Path::new("data"), path).is_err(), "{path}");
        }
        for args in [
            vec!["build-realm"],
            vec!["build-realm", "--client", "x"],
            vec!["build-realm", "--data"],
            vec!["build-realm", "--data", "data", "--id", "x"],
            vec!["build-realm", "--check", "--check"],
        ] {
            assert!(run(&args.into_iter().map(str::to_string).collect::<Vec<_>>()).is_err());
        }
        assert!(!run(&[]).unwrap());
        assert!(!run(&["backup".into()]).unwrap());
    }

    #[test]
    fn realm_prepare_stable_recipe_ids() {
        for id in [
            "world2",
            "synthetic-world02-copy",
            "classic-world-02-reconstruction",
        ] {
            let mut v = recipe_value();
            v["id"] = serde_json::json!(id);
            v["output"] = serde_json::json!(format!("{id}.wmap"));
            assert_eq!(recipe(v).unwrap().id, id);
        }
        for id in [
            "",
            ".",
            "..",
            "../escaped",
            "/absolute",
            "nested/world2",
            "nested\\world2",
            "World2",
            "world_2",
            "world.2",
            "world 2",
            "wörld2",
            "-world2",
            "world2-",
            "world--2",
            "world2\n",
        ] {
            let mut v = recipe_value();
            v["id"] = serde_json::json!(id);
            v["output"] = serde_json::json!(format!("{id}.wmap"));
            assert!(recipe(v).is_err(), "{id:?}");
        }
    }

    #[test]
    fn realm_prepare_recipe_discovery_symlinks_and_file_bounds() {
        let root = std::env::temp_dir().join(format!("realm-prepare-{}", rand::random::<u64>()));
        std::fs::create_dir(&root).unwrap();
        let write = |name: &str, v: &serde_json::Value| {
            std::fs::write(root.join(name), serde_json::to_vec(v).unwrap()).unwrap()
        };
        write("a.json", &recipe_value());
        assert_eq!(recipes(&root).unwrap().len(), 1);
        let mut synthetic = recipe_value();
        synthetic["id"] = serde_json::json!("synthetic-world02-copy");
        synthetic["output"] = serde_json::json!("synthetic-world02-copy.wmap");
        synthetic["provenance"]["status"] =
            serde_json::json!("synthetic World 2 copy for test only");
        write("b.json", &synthetic);
        let found = recipes(&root).unwrap();
        assert_eq!(
            found.keys().map(String::as_str).collect::<Vec<_>>(),
            ["classic-world-02-reconstruction", "synthetic-world02-copy"]
        );
        for (id, recipe) in &found {
            assert_eq!(recipe.output, format!("{id}.wmap"));
            recipe.validate().unwrap();
        }
        write("b.json", &recipe_value());
        assert!(
            recipes(&root)
                .unwrap_err()
                .to_string()
                .contains("duplicate")
        );
        std::fs::remove_file(root.join("b.json")).unwrap();
        let mut v = recipe_value();
        v["layers"]["elevation"]["path"] = serde_json::json!("../escaped");
        write("a.json", &v);
        assert!(recipes(&root).is_err());
        std::fs::write(root.join("a.json"), vec![b' '; 65537]).unwrap();
        assert!(recipes(&root).is_err());
        std::fs::write(root.join("small"), [0; 9]).unwrap();
        assert!(bounded_read(&root.join("small"), 8).is_err());
        assert!(bounded_read(&root, 8).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            write("a.json", &recipe_value());
            symlink("small", root.join("linked")).unwrap();
            assert!(safe_path(&root, "linked").is_err());
            std::fs::remove_file(root.join("a.json")).unwrap();
            symlink("small", root.join("a.json")).unwrap();
            assert!(recipes(&root).is_err());
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn realm_prepare_deterministic_scenery_sizes_and_coverage() {
        let content = Content::load(Path::new("data")).unwrap();
        let mut sizes = BTreeSet::new();
        let mut types = BTreeSet::new();
        for ground in [0x60, 0xbd, 0x48, 0x46, 0x56, 0x57, 0x23, 0x47, 0x72] {
            let mut count = 0;
            for i in 0..10000u64 {
                let c = Cell {
                    ground,
                    object: None,
                    config: String::new(),
                    terrain: 0,
                    region: 0,
                };
                let (mut a, mut b) = (c.clone(), c);
                for cell in [&mut a, &mut b] {
                    super::super::decorate(cell, super::super::mix(20927 ^ i)).unwrap();
                }
                assert_eq!(
                    serde_json::to_vec(&a).unwrap(),
                    serde_json::to_vec(&b).unwrap()
                );
                if let Some(kind) = a.object {
                    count += 1;
                    types.insert(kind);
                    assert!(content.objects[&kind].static_object && !content.objects[&kind].enemy);
                    if !a.config.is_empty() {
                        sizes.insert(a.config);
                    }
                }
            }
            assert!(count >= 50, "no meaningful scenery on {ground:#x}");
        }
        assert_eq!(
            sizes,
            (130..=160)
                .step_by(5)
                .map(|n| format!("size:{n}"))
                .collect()
        );
        assert_eq!(
            types,
            [
                0x100, 0x101, 0x102, 0x103, 0x105, 0x106, 0x107, 0x108, 0x109, 0x110, 0x111, 0x112,
                0x113, 0x114, 0x185, 0x186, 0x187, 0x188, 0x189
            ]
            .into_iter()
            .chain(0x197..=0x19f)
            .chain(0x1a1..=0x1a8)
            .chain(0x1aa..=0x1b1)
            .chain(0x1b3..=0x1ba)
            .collect()
        );
        for ground in [0xbc, 0x73, 0xbe, 0x6a, 0xd0] {
            let mut c = cell(0, 0, 0).unwrap();
            c.ground = ground;
            super::super::decorate(&mut c, 0).unwrap();
            assert!(c.object.is_none());
        }
    }

    #[test]
    fn realm_prepare_dictionary_config_encoding_and_roundtrip() {
        let content = Content::load(Path::new("data")).unwrap();
        let mut map = Map {
            width: 4,
            height: 1,
            spawns: vec![],
            cells: vec![
                Cell {
                    ground: 0x23,
                    object: Some(0x100),
                    config: "size:150".into(),
                    terrain: 3,
                    region: 0,
                },
                cell(204, 0, 0).unwrap(),
                cell(0, 0, 0x90).unwrap(),
                cell(0, 0, 0x80).unwrap(),
            ],
        };
        let (bytes, count) = encode(&map, &content).unwrap();
        assert_eq!(count, 4);
        assert_eq!(
            signature(&Map::decode_cells(&bytes, &content).unwrap()),
            signature(&map)
        );
        assert!(Map::decode(&bytes, &content).is_err());
        assert_eq!(encode(&map, &content).unwrap().0, bytes);
        map.cells.reverse();
        let (reversed, _) = encode(&map, &content).unwrap();
        let mut a = Vec::new();
        let mut b = Vec::new();
        flate2::read::ZlibDecoder::new(&bytes[..])
            .read_to_end(&mut a)
            .unwrap();
        flate2::read::ZlibDecoder::new(&reversed[..])
            .read_to_end(&mut b)
            .unwrap();
        assert_eq!(&a[..a.len() - 8], &b[..b.len() - 8]);
        for config in [
            "size:129",
            "size:131",
            "size:165",
            "size:0150",
            "name:x",
            "size:150;name:x",
        ] {
            let mut bad = map.clone();
            bad.cells[3].config = config.into();
            assert!(encode(&bad, &content).is_err());
        }
        let mut long_content = content.clone();
        long_content.objects.get_mut(&0x100).unwrap().name = "a".repeat(130);
        let (long, _) = encode(&map, &long_content).unwrap();
        assert_eq!(
            signature(&Map::decode_cells(&long, &long_content).unwrap()),
            signature(&map)
        );
        long_content.objects.get_mut(&0x100).unwrap().name = "a".repeat(1025);
        assert!(encode(&map, &long_content).is_err());
        let mut duplicate = content.objects[&0x100].clone();
        duplicate.kind = 0xffff;
        let mut ambiguous = content.clone();
        ambiguous.objects.insert(0xffff, duplicate);
        assert!(encode(&map, &ambiguous).is_err());
        for (index, ground, object, config, terrain, region) in [
            (0, 0x08, None, "", 0, 0),
            (0, 0x71, None, "", 0, 0),
            (0, 0xbd, Some(0x600), "", 0, 0),
            (0, 0xbd, None, "size:150", 0, 0),
            (0, 0xbd, None, "", 13, 0),
            (0, 0xbd, None, "", 0, 1),
        ] {
            let mut bad = map.clone();
            bad.cells[index] = Cell {
                ground,
                object,
                config: config.into(),
                terrain,
                region,
            };
            assert!(encode(&bad, &content).is_err());
        }
    }

    #[test]
    fn realm_prepare_mainland_topology_preserves_lakes_channels_and_roads() {
        let mut o = vec![0x50; 35];
        for (i, code) in [
            (8, 0x80),
            (9, 0x10),
            (10, 0x60),
            (11, 0x40),
            (12, 0xc0),
            (15, 0),
            (16, 0x90),
            (17, 0x30),
            (18, 0),
            (19, 0x80),
            (28, 0x80),
        ] {
            o[i] = code;
        }
        let original = o
            .iter()
            .map(|&v| cell(0, 0, v).unwrap())
            .collect::<Vec<_>>();
        let mut cells = original.clone();
        let stats = normalize_mainland(&mut cells, &o, 7).unwrap();
        assert_eq!((stats.removed_components, stats.removed_cells), (1, 1));
        let mut expected = original;
        expected[28] = cell(0, 0, 0x50).unwrap();
        let map = |cells| Map {
            width: 7,
            height: 5,
            cells,
            spawns: vec![],
        };
        assert_eq!(signature(&map(cells)), signature(&map(expected)));
        let o = [0x50, 0x50, 0x50, 0x80, 0x80, 0x90, 0, 0x50];
        let mut cells = o
            .iter()
            .map(|&v| cell(0, 0, v).unwrap())
            .collect::<Vec<_>>();
        let stats = normalize_mainland(&mut cells, &o, 4).unwrap();
        assert_eq!(stats.removed_cells, 1);
        assert_eq!(cells[3].ground, 0xbc);
        assert_eq!(cells[4].ground, 0xbe);
        assert_eq!(cells[5].ground, 0xd0);
        for o in [[0x50; 3], [0x80, 0x50, 0x80]] {
            let mut cells = o
                .iter()
                .map(|&v| cell(0, 0, v).unwrap())
                .collect::<Vec<_>>();
            assert!(normalize_mainland(&mut cells, &o, 3).is_err());
        }
        assert!(assemble(&[0; 5], &[0; 5], &[0, 0, 0x50, 0x50, 0xd0], 5, 1, 0).is_err());
    }

    #[test]
    fn realm_prepare_frozen_world02_full_array_and_geography() {
        let root = Path::new("data/realms");
        let recipes = recipes(root).unwrap();
        let r = &recipes["classic-world-02-reconstruction"];
        let e = load_layer(root, &r.layers.elevation).unwrap();
        let m = load_layer(root, &r.layers.moisture).unwrap();
        let o = load_layer(root, &r.layers.overrides).unwrap();
        let (map, _) = assemble(&e, &m, &o, SIDE, SIDE, r.scenery_seed).unwrap();
        let row_overrides = transpose(&o, SIDE, SIDE).unwrap();
        let mut expected = (0..CELLS)
            .map(|i| {
                let j = (i % SIDE) * SIDE + i / SIDE;
                cell(e[j], m[j], o[j]).unwrap()
            })
            .collect::<Vec<_>>();
        normalize_mainland(&mut expected, &row_overrides, SIDE).unwrap();
        let mut habitats = BTreeMap::<u8, usize>::new();
        let mut configured = 0;
        let mut scenery = 0;
        for (i, c) in map.cells.iter().enumerate() {
            let source = (i % SIDE) * SIDE + i / SIDE;
            let base = &expected[i];
            assert_eq!(c.ground, base.ground, "ground {},{}", i % SIDE, i / SIDE);
            assert!(
                c.terrain == base.terrain || (c.terrain == 12 && matches!(base.terrain, 9 | 10))
            );
            assert_eq!(c.region, 0);
            if o[source] >> 4 != 0 && o[source] >> 4 != 7 {
                assert!(c.object.is_none());
            }
            if c.object.is_some() {
                scenery += 1;
            }
            if !c.config.is_empty() {
                configured += 1;
            }
            *habitats.entry(c.terrain).or_default() += 1;
        }
        for terrain in 1..=12 {
            assert!(habitats[&terrain] > 1000, "habitat {terrain}");
        }
        assert!(scenery > 100000 && configured > 20000);
        // Landmark constraints are reconstruction fixtures, not claims of original tile identity.
        for (x, y, ground, terrain) in [
            (1320, 60, 0xbe, 11),
            (870, 780, 0xbc, 0),
            (1090, 755, 0x60, 1),
            (1200, 1100, 0x60, 1),
            (1051, 768, 0x6a, 1),
            (350, 784, 0xd0, 0),
            (1238, 1600, 0xd0, 0),
            (960, 950, 0x73, 0),
            (1430, 1950, 0xbe, 11),
            (700, 100, 0xbc, 0),
        ] {
            let c = &map.cells[y * SIDE + x];
            assert_eq!((c.ground, c.terrain), (ground, terrain), "landmark {x},{y}");
        }
        assert_eq!(scenery, 322240);
        assert_eq!(configured, 151504);
        assert_eq!(
            habitats,
            [
                (0, 2140215),
                (1, 84872),
                (2, 89359),
                (3, 100066),
                (4, 14247),
                (5, 227456),
                (6, 283191),
                (7, 285097),
                (8, 294207),
                (9, 90559),
                (10, 374427),
                (11, 110941),
                (12, 99667)
            ]
            .into_iter()
            .collect()
        );
        let content = Content::load(Path::new("data")).unwrap();
        let (bytes, dictionary) = encode(&map, &content).unwrap();
        assert!(dictionary > 100);
        let decoded = Map::decode_cells(&bytes, &content).unwrap();
        assert_eq!(signature(&decoded), signature(&map));
        assert_eq!(bytes, std::fs::read(root.join(&r.output)).unwrap());
    }
}
