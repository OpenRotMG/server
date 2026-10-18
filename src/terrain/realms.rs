use crate::content::{Content, Map};
use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use std::{collections::VecDeque, path::Path, sync::Arc};

use super::prepare::{bounded_read, read_recipe, safe_path, valid_hash, valid_id};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Entry {
    Baseline {
        id: String,
        path: String,
        sha256: String,
    },
    Reconstruction {
        recipe: String,
        sha256: String,
    },
}

pub(crate) struct Asset {
    pub(crate) id: String,
    pub(crate) bytes: Vec<u8>,
    sha256: String,
    profile: String,
}

pub(crate) enum Source {
    Encoded(Asset),
    #[cfg(test)]
    Fixture(Arc<Map>),
}
impl Source {
    #[cfg(test)]
    pub(crate) fn encoded_fixture(id: &str, bytes: Vec<u8>) -> Self {
        Self::Encoded(Asset {
            id: id.into(),
            sha256: format!("{:x}", Sha256::digest(&bytes)),
            bytes,
            profile: "test".into(),
        })
    }
    pub(crate) fn name(&self) -> &str {
        match self {
            Self::Encoded(asset) => &asset.id,
            #[cfg(test)]
            Self::Fixture(_) => "procedural-test-fixture",
        }
    }
    pub(crate) fn open(&self, content: &Content) -> Result<Arc<Map>> {
        match self {
            Self::Encoded(asset) => {
                let name = &asset.id;
                let bytes = &asset.bytes;
                let started = std::time::Instant::now();
                let mut map = Map::decode_cells(bytes, content)
                    .with_context(|| format!("decode Realm layout {name}"))?;
                ensure!(
                    map.width == 2048 && map.height == 2048,
                    "{name}: expected 2048-square layout"
                );
                prepare_arrivals(&mut map, content)
                    .with_context(|| format!("prepare Realm layout {name}"))?;
                tracing::info!(
                    layout = name,
                    sha256 = asset.sha256,
                    profile = asset.profile,
                    width = map.width,
                    height = map.height,
                    scenery = map.cells.iter().filter(|c| c.object.is_some()).count(),
                    elapsed_ms = started.elapsed().as_millis(),
                    "Realm layout opened"
                );
                Ok(Arc::new(map))
            }
            #[cfg(test)]
            Self::Fixture(map) => Ok(map.clone()),
        }
    }
}
pub(crate) fn sources(path: &Path) -> Result<Vec<Source>> {
    let catalog = safe_path(path, "realm-catalog.json")?;
    let entries: Vec<Entry> = serde_json::from_slice(&bounded_read(&catalog, 64 * 1024)?)
        .with_context(|| format!("parse Realm catalog {}", catalog.display()))?;
    ensure!(
        !entries.is_empty(),
        "empty Realm catalog {}",
        catalog.display()
    );
    let mut ids = std::collections::BTreeSet::new();
    entries
        .into_iter()
        .enumerate()
        .map(|(index, entry)| {
            let load = || -> Result<Source> {
                let (id, file, expected, profile) = match entry {
                    Entry::Baseline {
                        id,
                        path: relative,
                        sha256,
                    } => (id, safe_path(path, &relative)?, sha256, "fsod".to_owned()),
                    Entry::Reconstruction { recipe, sha256 } => {
                        let root = safe_path(path, "realms")?;
                        let metadata = read_recipe(&root, &recipe).with_context(|| {
                            format!("Realm recipe {recipe} expected output SHA-256 {sha256}")
                        })?;
                        let file = safe_path(&root, &metadata.output)?;
                        (
                            metadata.id,
                            file,
                            sha256,
                            metadata.profile.name().to_owned(),
                        )
                    }
                };
                ensure!(valid_id(&id), "invalid Realm catalog ID {id:?}");
                ensure!(ids.insert(id.clone()), "duplicate Realm catalog ID {id}");
                ensure!(
                    valid_hash(&expected),
                    "Realm {id} {}: invalid SHA-256 {expected:?}",
                    file.display()
                );
                let bytes = bounded_read(&file, 16 * 1024 * 1024).with_context(|| {
                    format!("Realm {id} {} expected SHA-256 {expected}", file.display())
                })?;
                let hash = format!("{:x}", Sha256::digest(&bytes));
                ensure!(
                    hash == expected,
                    "Realm {id} {} hash mismatch: expected {expected}, got {hash}",
                    file.display()
                );
                tracing::info!(layout = id, sha256 = hash, profile, "Realm asset verified");
                Ok(Source::Encoded(Asset {
                    id,
                    bytes,
                    sha256: hash,
                    profile,
                }))
            };
            load().with_context(|| format!("Realm catalog {} entry {index}", catalog.display()))
        })
        .collect()
}

fn prepare_arrivals(map: &mut Map, content: &Content) -> Result<()> {
    let width = map.width as usize;
    let passable: Vec<_> = map
        .cells
        .iter()
        .map(|c| {
            content.grounds[&c.ground] > 0.
                && c.object.is_none_or(|kind| !content.objects[&kind].blocks)
        })
        .collect();
    let start = map
        .cells
        .iter()
        .enumerate()
        .filter(|(i, c)| c.terrain == 1 && passable[*i])
        .min_by_key(|(i, _)| {
            (i % width).abs_diff(width / 2) + (i / width).abs_diff(map.height as usize / 2)
        })
        .map(|(i, _)| i)
        .context("Realm has no walkable Godlands")?;
    // A remote beach island is not a usable arrival even when its surrounding nine cells are clear.
    let mut reachable = vec![false; map.cells.len()];
    let mut queue = VecDeque::from([start]);
    reachable[start] = true;
    while let Some(i) = queue.pop_front() {
        let x = i % width;
        for next in [
            i.wrapping_sub(width),
            i + width,
            if x > 0 { i - 1 } else { usize::MAX },
            if x + 1 < width { i + 1 } else { usize::MAX },
        ] {
            if next < reachable.len() && passable[next] && !reachable[next] {
                reachable[next] = true;
                queue.push_back(next);
            }
        }
    }
    for cell in &mut map.cells {
        if cell.region == 1 {
            cell.region = 0;
        }
    }
    map.spawns.clear();
    for (dx, dy) in [(0, 1), (1, 0), (0, -1), (-1, 0)] {
        let (x, y) = (1..map.height - 1)
            .flat_map(|y| (1..map.width - 1).map(move |x| (x, y)))
            .filter(|(x, y)| {
                let (rx, ry) = (x - map.width / 2, y - map.height / 2);
                dx * rx + dy * ry > (dx * ry - dy * rx).abs()
                    && reachable[(y * map.width + x) as usize]
                    && (y - 1..=y + 1).all(|ny| {
                        (x - 1..=x + 1).all(|nx| {
                            let cell = &map.cells[(ny * map.width + nx) as usize];
                            cell.terrain == 11 && content.grounds[&cell.ground] > 0.
                        })
                    })
            })
            .min_by_key(|(x, y)| {
                let (rx, ry) = (x - map.width / 2, y - map.height / 2);
                ((dx * ry - dy * rx).abs(), -(dx * rx + dy * ry))
            })
            .with_context(|| format!("Realm has no connected coastal arrival toward {dx},{dy}"))?;
        for ny in y - 1..=y + 1 {
            for nx in x - 1..=x + 1 {
                let cell = &mut map.cells[(ny * map.width + nx) as usize];
                cell.object = None;
                cell.config.clear();
                cell.region = 1;
            }
        }
        map.spawns.push((x as f32 + 0.5, y as f32 + 0.5));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::Cell;
    use std::io::Write;

    fn marker_free_realm_bytes(godlands: bool, connected: bool) -> Vec<u8> {
        let mut raw = Vec::new();
        raw.extend(4i16.to_le_bytes());
        for (ground, name, config, terrain) in [
            (0xbc, "", "", 0),
            (0xbd, "", "", 0),
            (0x60, "", "", u8::from(godlands)),
            (0xbe, "Bush", "size:150", 11),
        ] {
            raw.extend((ground as i16).to_le_bytes());
            for text in [name, config] {
                raw.push(text.len() as u8);
                raw.extend(text.as_bytes());
            }
            raw.extend([terrain, 0]);
        }
        raw.extend(2048i32.to_le_bytes());
        raw.extend(2048i32.to_le_bytes());
        for y in 0i32..2048 {
            for x in 0i32..2048 {
                let index: i16 = if [(1024, 1984), (1984, 1024), (1024, 64), (64, 1024)]
                    .iter()
                    .any(|&(cx, cy)| (x - cx).abs() <= 1 && (y - cy).abs() <= 1)
                {
                    3
                } else if (x, y) == (1024, 1024) {
                    2
                } else if connected
                    && ((x == 1024 && (64..=1984).contains(&y))
                        || (y == 1024 && (64..=1984).contains(&x)))
                {
                    1
                } else {
                    0
                };
                raw.extend(index.to_le_bytes());
            }
        }
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&raw).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn marker_free_realm_prepares_arrivals() {
        let content = Content::load(Path::new("data")).unwrap();
        assert!(!content.objects[&0x105].blocks);
        let source =
            Source::encoded_fixture("marker-free-test", marker_free_realm_bytes(true, true));
        let map = source.open(&content).unwrap();
        assert_eq!((map.width, map.height), (2048, 2048));
        assert_eq!(
            map.spawns,
            [
                (1024.5, 1984.5),
                (1984.5, 1024.5),
                (1024.5, 64.5),
                (64.5, 1024.5)
            ]
        );
        assert_eq!(map.cells.iter().filter(|c| c.region == 1).count(), 36);
        assert!(
            map.cells
                .iter()
                .all(|c| c.object.is_none() && c.config.is_empty())
        );
        assert_eq!(map.cell(1024.5, 1024.5).unwrap().terrain, 1);
        for &(x, y) in &map.spawns {
            for ny in y as i32 - 1..=y as i32 + 1 {
                for nx in x as i32 - 1..=x as i32 + 1 {
                    let cell = map.cell(nx as f32, ny as f32).unwrap();
                    assert_eq!((cell.terrain, cell.region), (11, 1));
                }
            }
            for step in 0..=(x as i32 - 1024).abs().max((y as i32 - 1024).abs()) {
                let nx = x as i32 + (1024 - x as i32).signum() * step;
                let ny = y as i32 + (1024 - y as i32).signum() * step;
                let cell = map.cell(nx as f32, ny as f32).unwrap();
                assert!(content.grounds[&cell.ground] > 0. && cell.object.is_none());
            }
        }
    }

    #[test]
    fn ordinary_map_still_requires_spawns() {
        let content = Content::load(Path::new("data")).unwrap();
        let bytes = marker_free_realm_bytes(true, true);
        let cells = Map::decode_cells(&bytes, &content).unwrap();
        assert!(cells.spawns.is_empty());
        assert!(cells.cells.iter().all(|c| c.region == 0));
        assert_eq!(
            cells.cells.iter().filter(|c| c.object.is_some()).count(),
            36
        );
        drop(cells);
        let error = Map::decode(&bytes, &content).unwrap_err().to_string();
        assert!(error.contains("map has no spawn cells"), "{error}");
        let path = std::env::temp_dir().join(format!(
            "openrotmg-marker-free-{}.wmap",
            rand::random::<u64>()
        ));
        std::fs::write(&path, &bytes).unwrap();
        let error = format!("{:#}", Map::load(&path, &content).unwrap_err());
        std::fs::remove_file(path).unwrap();
        assert!(error.contains("map has no spawn cells"), "{error}");
    }

    #[test]
    fn encoded_realms_reject_missing_godlands_and_disconnected_beaches() {
        let content = Content::load(Path::new("data")).unwrap();
        for (godlands, connected, expected) in [
            (false, true, "Realm has no walkable Godlands"),
            (true, false, "Realm has no connected coastal arrival"),
        ] {
            let source = Source::encoded_fixture(
                "invalid-test",
                marker_free_realm_bytes(godlands, connected),
            );
            let error = format!("{:#}", source.open(&content).unwrap_err());
            assert!(
                error.contains("prepare Realm layout invalid-test"),
                "{error}"
            );
            assert!(error.contains(expected), "{error}");
        }
    }

    #[test]
    fn arrivals_reject_remote_islands_and_missing_habitats() {
        let content = Content::load(Path::new("data")).unwrap();
        let mut map = Map {
            width: 64,
            height: 64,
            spawns: vec![],
            cells: vec![
                Cell {
                    ground: 0xbc,
                    object: None,
                    config: String::new(),
                    terrain: 0,
                    region: 0
                };
                64 * 64
            ],
        };
        for (x0, y0, size) in [
            (16, 16, 32),
            (30, 4, 5),
            (30, 55, 5),
            (4, 30, 5),
            (55, 30, 5),
        ] {
            for y in y0..y0 + size {
                for x in x0..x0 + size {
                    let cell = &mut map.cells[y * 64 + x];
                    cell.ground = 0xbe;
                    cell.terrain = 11;
                    cell.region = 1;
                }
            }
        }
        map.cells[32 * 64 + 32].terrain = 1;
        prepare_arrivals(&mut map, &content).unwrap();
        assert_eq!(map.spawns.len(), 4);
        assert!(
            map.spawns
                .iter()
                .all(|(x, y)| (17.0..47.0).contains(x) && (17.0..47.0).contains(y))
        );
        assert_eq!(map.cells.iter().filter(|c| c.region == 1).count(), 36);
        map.cells[32 * 64 + 32].terrain = 0;
        assert!(
            prepare_arrivals(&mut map, &content)
                .unwrap_err()
                .to_string()
                .contains("Godlands")
        );
    }
    #[test]
    fn catalog_startup_rejects_malformed_entries_and_every_missing_or_modified_dependency() {
        let root =
            std::env::temp_dir().join(format!("openrotmg-catalog-{}", rand::random::<u64>()));
        std::fs::create_dir_all(root.join("realms")).unwrap();
        let mut dependencies = vec!["realm-catalog.json".to_owned()];
        let catalog: Vec<Entry> =
            serde_json::from_slice(&std::fs::read("data/realm-catalog.json").unwrap()).unwrap();
        for entry in catalog {
            match entry {
                Entry::Baseline { path, .. } => dependencies.push(path),
                Entry::Reconstruction { recipe, .. } => {
                    let metadata = read_recipe(Path::new("data/realms"), &recipe).unwrap();
                    dependencies.push(format!("realms/{recipe}"));
                    dependencies.push(format!("realms/{}", metadata.output));
                }
            }
        }
        for relative in &dependencies {
            std::fs::copy(Path::new("data").join(relative), root.join(relative)).unwrap();
        }
        let content = Arc::new(Content::load(Path::new("data")).unwrap());
        let error = || {
            let result = crate::world::Directory::with_seed(&root, content.clone(), 8, 0);
            format!(
                "{:#}",
                result
                    .err()
                    .expect("startup must reject catalog dependency")
            )
        };
        for relative in &dependencies {
            let file = root.join(relative);
            let original = std::fs::read(&file).unwrap();
            std::fs::remove_file(&file).unwrap();
            let missing = error();
            assert!(missing.contains(relative), "{missing}");
            std::fs::write(&file, b"modified").unwrap();
            let modified = error();
            assert!(modified.contains(relative), "{modified}");
            if relative.ends_with(".wmap") {
                let id = file.file_stem().unwrap().to_str().unwrap();
                assert!(
                    missing.contains(id) && missing.contains("expected SHA-256"),
                    "{missing}"
                );
                assert!(
                    modified.contains(id)
                        && modified.contains("hash mismatch: expected")
                        && modified.contains("got"),
                    "{modified}"
                );
            }
            std::fs::write(file, original).unwrap();
        }
        let file = root.join("realm-catalog.json");
        let original: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        for (label, expected) in [
            ("empty", "empty Realm catalog"),
            ("empty-id", "invalid Realm catalog ID"),
            ("unsafe-id", "invalid Realm catalog ID"),
            ("duplicate", "duplicate Realm catalog ID realm-fsod-1"),
            (
                "cross-kind-duplicate",
                "duplicate Realm catalog ID realm-fsod-1",
            ),
            ("unknown-field", "unknown field"),
            ("unknown-kind", "unknown variant"),
            ("missing-hash", "missing field"),
            ("bad-hash", "invalid SHA-256"),
            ("unsafe-path", "unsafe recipe path"),
            ("bad-profile", "unknown variant"),
        ] {
            let mut value = original.clone();
            match label {
                "empty" => value = serde_json::json!([]),
                "empty-id" => value[0]["id"] = "".into(),
                "unsafe-id" => value[0]["id"] = "../escape".into(),
                "duplicate" => value[1]["id"] = "realm-fsod-1".into(),
                "cross-kind-duplicate" | "bad-profile" => {
                    let recipe_file = root.join("realms/world02.json");
                    let mut recipe: serde_json::Value =
                        serde_json::from_slice(&std::fs::read(&recipe_file).unwrap()).unwrap();
                    if label == "cross-kind-duplicate" {
                        recipe["id"] = "realm-fsod-1".into();
                        recipe["output"] = "realm-fsod-1.wmap".into();
                    } else {
                        recipe["profile"] = "unknown".into();
                    }
                    std::fs::write(recipe_file, serde_json::to_vec(&recipe).unwrap()).unwrap();
                }
                "unknown-field" => value[0]["ignored"] = true.into(),
                "unknown-kind" => value[0]["kind"] = "unknown".into(),
                "missing-hash" => {
                    value[0].as_object_mut().unwrap().remove("sha256");
                }
                "bad-hash" => value[0]["sha256"] = "not-a-hash".into(),
                "unsafe-path" => value[0]["path"] = "../escape.wmap".into(),
                _ => unreachable!(),
            }
            std::fs::write(&file, serde_json::to_vec(&value).unwrap()).unwrap();
            let rejection = error();
            assert!(rejection.contains(expected), "{label}: {rejection}");
            std::fs::copy("data/realms/world02.json", root.join("realms/world02.json")).unwrap();
        }
        std::fs::write(&file, serde_json::to_vec(&original).unwrap()).unwrap();
        // Compilation metadata must remain readable before an output exists.
        std::fs::remove_file(root.join("realms/classic-world-02-reconstruction.wmap")).unwrap();
        let recipe = read_recipe(&root.join("realms"), "world02.json").unwrap();
        assert_eq!(recipe.id, "classic-world-02-reconstruction");
        std::fs::remove_dir_all(root).unwrap();
    }
}
