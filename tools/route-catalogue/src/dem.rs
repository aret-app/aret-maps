//! Elevation from the TINITALY digital elevation model (INGV, 10 m, CC BY 4.0; aret-maps#5).
//!
//! TINITALY 1.1 comes as GeoTIFF tiles of 50 × 50 km in UTM zone 32 (EPSG:32632): uncompressed
//! 32-bit floats, metres above sea level, `-9999` where there is no data. This module reads exactly
//! that format (and refuses anything else) and samples it with bilinear interpolation, reading only
//! the few cells it needs from disk, so memory stays small however many tiles there are.

use std::fs::File;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};

use proj4rs::proj::Proj;

/// Citation TINITALY's licence (CC BY 4.0) asks for.
pub const CITATION: &str = "Tarquini S., Isola I., Favalli M., Battistini A., Dotta G. (2023). \
     TINITALY, a digital elevation model of Italy with a 10 meters cell size (Version 1.1). \
     Istituto Nazionale di Geofisica e Vulcanologia (INGV). https://doi.org/10.13127/tinitaly/1.1";

/// EPSG code of UTM zone 32 north on WGS 84, TINITALY's grid.
const EPSG_UTM_32N: u16 = 32632;

/// The elevation model could not be read.
#[must_use]
#[derive(Debug)]
pub struct DemError(pub String);

impl std::fmt::Display for DemError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for DemError {}

/// One GeoTIFF tile: where it is, and where each row starts in the file.
struct Tile {
    path: PathBuf,
    file: File,
    width: usize,
    height: usize,
    /// UTM easting of the left edge and northing of the top edge, in metres.
    left: f64,
    top: f64,
    /// Cell size in metres (10 for TINITALY).
    cell: f64,
    nodata: f32,
    /// File offset of the first value of each strip, and rows per strip.
    strip_offsets: Vec<u64>,
    rows_per_strip: usize,
}

/// All tiles of a folder, plus the WGS 84 → UTM 32N conversion.
pub struct Dem {
    tiles: Vec<Tile>,
    geographic: Proj,
    utm: Proj,
}

impl Dem {
    /// Opens every `.tif` in `folder` (searched recursively, as the tiles come one per subfolder).
    pub fn open(folder: &Path) -> Result<Self, DemError> {
        let mut paths = Vec::new();
        collect_tiffs(folder, &mut paths)?;
        paths.sort();
        if paths.is_empty() {
            return Err(DemError(format!(
                "no .tif tiles in {} (run scripts/fetch-dem.sh)",
                folder.display()
            )));
        }
        let tiles = paths
            .iter()
            .map(|path| Tile::open(path))
            .collect::<Result<Vec<Tile>, DemError>>()?;
        let projection = |definition: &str| {
            Proj::from_proj_string(definition)
                .map_err(|error| DemError(format!("projection {definition}: {error:?}")))
        };
        Ok(Self {
            tiles,
            geographic: projection("+proj=longlat +datum=WGS84 +no_defs")?,
            utm: projection("+proj=utm +zone=32 +datum=WGS84 +units=m +no_defs")?,
        })
    }

    /// Number of tiles.
    pub fn tile_count(&self) -> usize {
        self.tiles.len()
    }

    /// Elevation in metres at a WGS 84 position, or `None` outside the tiles or on missing data.
    pub fn elevation(&self, (lon, lat): (f64, f64)) -> Option<f64> {
        let (easting, northing) = self.to_utm((lon, lat))?;
        self.tiles
            .iter()
            .find_map(|tile| tile.sample(easting, northing))
    }

    /// WGS 84 longitude/latitude in degrees → UTM 32N easting/northing in metres.
    pub fn to_utm(&self, (lon, lat): (f64, f64)) -> Option<(f64, f64)> {
        let mut point = (lon.to_radians(), lat.to_radians());
        proj4rs::transform::transform(&self.geographic, &self.utm, &mut point).ok()?;
        Some(point)
    }
}

/// Names of the TINITALY 1.1 tiles covering `area`, e.g. `w49060_s10`: zone 32 ("w"), the 50 km
/// square starting 4,900 km north and 600 km east. Some names may not exist (sea-only squares).
pub fn tile_names(area: &crate::catalogue::BoundingBox) -> Result<Vec<String>, DemError> {
    let projection = |definition: &str| {
        Proj::from_proj_string(definition)
            .map_err(|error| DemError(format!("projection {definition}: {error:?}")))
    };
    let geographic = projection("+proj=longlat +datum=WGS84 +no_defs")?;
    let utm = projection("+proj=utm +zone=32 +datum=WGS84 +units=m +no_defs")?;
    let mut names = std::collections::BTreeSet::new();
    // A grid of points every ~1 km over the area: a square of 50 km can't fall between them
    let steps = 200;
    for i in 0..=steps {
        for j in 0..=steps {
            let lon = area.west + (area.east - area.west) * f64::from(i) / f64::from(steps);
            let lat = area.south + (area.north - area.south) * f64::from(j) / f64::from(steps);
            let mut point = (lon.to_radians(), lat.to_radians());
            proj4rs::transform::transform(&geographic, &utm, &mut point)
                .map_err(|error| DemError(format!("projecting {lon},{lat}: {error:?}")))?;
            let north = (point.1 / 50_000.0).floor() as i64 * 5;
            let east = (point.0 / 50_000.0).floor() as i64 * 5;
            names.insert(format!("w{north:03}{east:02}_s10"));
        }
    }
    Ok(names.into_iter().collect())
}

fn collect_tiffs(folder: &Path, paths: &mut Vec<PathBuf>) -> Result<(), DemError> {
    let entries = std::fs::read_dir(folder)
        .map_err(|error| DemError(format!("{}: {error}", folder.display())))?;
    for entry in entries {
        let path = entry
            .map_err(|error| DemError(format!("{}: {error}", folder.display())))?
            .path();
        if path.is_dir() {
            collect_tiffs(&path, paths)?;
        } else if path.extension().is_some_and(|ext| ext == "tif") {
            paths.push(path);
        }
    }
    Ok(())
}

impl Tile {
    fn open(path: &Path) -> Result<Self, DemError> {
        let fail = |what: &str| DemError(format!("{}: {what}", path.display()));
        let file = File::open(path).map_err(|error| fail(&error.to_string()))?;
        let header = Header::read(&file).map_err(|what| fail(&what))?;

        // Only TINITALY's exact layout: anything else would be misread silently
        let width = header.number(256).ok_or_else(|| fail("no width"))? as usize;
        let height = header.number(257).ok_or_else(|| fail("no height"))? as usize;
        if header.number(258) != Some(32) || header.number(339) != Some(3) {
            return Err(fail("expected 32-bit float cells"));
        }
        if header.number(259) != Some(1) {
            return Err(fail("expected an uncompressed file"));
        }
        if header.number(277).unwrap_or(1) != 1 || header.number(284).unwrap_or(1) != 1 {
            return Err(fail("expected one band"));
        }
        if !header.geokeys().contains(&u64::from(EPSG_UTM_32N)) {
            return Err(fail("expected UTM zone 32N (EPSG:32632)"));
        }
        let scale = header.doubles(33550);
        let tiepoint = header.doubles(33922);
        let ([cell_x, cell_y, ..], [0.0, 0.0, _, left, top, ..]) =
            (scale.as_slice(), tiepoint.as_slice())
        else {
            return Err(fail("missing or unusual georeference"));
        };
        if (cell_x - cell_y).abs() > 1e-9 {
            return Err(fail("expected square cells"));
        }
        let strip_offsets = header.numbers(273);
        let rows_per_strip = header.number(278).unwrap_or(height as u64) as usize;
        if strip_offsets.len() != height.div_ceil(rows_per_strip) {
            return Err(fail("unexpected number of strips"));
        }
        let nodata: f32 = header
            .text(42113)
            .and_then(|text| text.trim_end_matches('\0').trim().parse().ok())
            .unwrap_or(-9999.0);
        Ok(Self {
            path: path.to_owned(),
            file,
            width,
            height,
            left: *left,
            top: *top,
            cell: *cell_x,
            nodata,
            strip_offsets,
            rows_per_strip,
        })
    }

    /// Bilinear interpolation between the four cell centres around the point, or `None` if the
    /// point isn't well inside this tile or a cell has no data.
    fn sample(&self, easting: f64, northing: f64) -> Option<f64> {
        let x = (easting - self.left) / self.cell - 0.5;
        let y = (self.top - northing) / self.cell - 0.5;
        if x < 0.0 || y < 0.0 {
            return None;
        }
        let (column, row) = (x.floor() as usize, y.floor() as usize);
        if column + 1 >= self.width || row + 1 >= self.height {
            return None;
        }
        let (dx, dy) = (x - column as f64, y - row as f64);
        let [a, b] = self.pair(row, column)?;
        let [c, d] = self.pair(row + 1, column)?;
        let top = a * (1.0 - dx) + b * dx;
        let bottom = c * (1.0 - dx) + d * dx;
        Some(top * (1.0 - dy) + bottom * dy)
    }

    /// Two neighbouring cells of a row, read straight from the file.
    fn pair(&self, row: usize, column: usize) -> Option<[f64; 2]> {
        let strip = self.strip_offsets[row / self.rows_per_strip];
        let offset = strip + (((row % self.rows_per_strip) * self.width + column) * 4) as u64;
        let mut bytes = [0u8; 8];
        if let Err(error) = self.file.read_exact_at(&mut bytes, offset) {
            eprintln!("{}: {error}", self.path.display());
            return None;
        }
        let first = f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let second = f32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        let valid =
            |value: f32| (value != self.nodata && value > -1000.0).then_some(f64::from(value));
        Some([valid(first)?, valid(second)?])
    }
}

/// The tags of a little-endian classic TIFF's first image.
struct Header {
    entries: Vec<(u16, Value)>,
}

enum Value {
    Numbers(Vec<u64>),
    Doubles(Vec<f64>),
    Text(String),
}

impl Header {
    fn read(file: &File) -> Result<Self, String> {
        let read = |offset: u64, length: usize| -> Result<Vec<u8>, String> {
            let mut buffer = vec![0u8; length];
            file.read_exact_at(&mut buffer, offset)
                .map_err(|error| format!("reading the header: {error}"))?;
            Ok(buffer)
        };
        let head = read(0, 8)?;
        if &head[..4] != b"II\x2a\x00" {
            return Err("expected a little-endian classic TIFF".to_owned());
        }
        let ifd = u64::from(u32::from_le_bytes([head[4], head[5], head[6], head[7]]));
        let count_bytes = read(ifd, 2)?;
        let count = usize::from(u16::from_le_bytes([count_bytes[0], count_bytes[1]]));
        let raw = read(ifd + 2, count * 12)?;

        let mut entries = Vec::with_capacity(count);
        for entry in raw.as_chunks::<12>().0 {
            let tag = u16::from_le_bytes([entry[0], entry[1]]);
            let kind = u16::from_le_bytes([entry[2], entry[3]]);
            let n = u32::from_le_bytes([entry[4], entry[5], entry[6], entry[7]]) as usize;
            let unit = match kind {
                1 | 2 => 1,
                3 => 2,
                4 => 4,
                12 => 8,
                _ => continue, // types we never need
            };
            let data = if unit * n <= 4 {
                entry[8..8 + unit * n].to_vec()
            } else {
                let at = u32::from_le_bytes([entry[8], entry[9], entry[10], entry[11]]);
                read(u64::from(at), unit * n)?
            };
            let value = match kind {
                2 => Value::Text(String::from_utf8_lossy(&data).into_owned()),
                3 => Value::Numbers(
                    data.as_chunks::<2>()
                        .0
                        .iter()
                        .map(|b| u64::from(u16::from_le_bytes(*b)))
                        .collect(),
                ),
                4 => Value::Numbers(
                    data.as_chunks::<4>()
                        .0
                        .iter()
                        .map(|b| u64::from(u32::from_le_bytes(*b)))
                        .collect(),
                ),
                12 => Value::Doubles(
                    data.as_chunks::<8>()
                        .0
                        .iter()
                        .map(|b| f64::from_le_bytes(*b))
                        .collect(),
                ),
                _ => Value::Numbers(data.iter().map(|&byte| u64::from(byte)).collect()),
            };
            entries.push((tag, value));
        }
        Ok(Self { entries })
    }

    fn get(&self, tag: u16) -> Option<&Value> {
        self.entries
            .iter()
            .find(|(t, _)| *t == tag)
            .map(|(_, value)| value)
    }

    fn numbers(&self, tag: u16) -> Vec<u64> {
        match self.get(tag) {
            Some(Value::Numbers(values)) => values.clone(),
            _ => Vec::new(),
        }
    }

    fn number(&self, tag: u16) -> Option<u64> {
        self.numbers(tag).first().copied()
    }

    fn doubles(&self, tag: u16) -> Vec<f64> {
        match self.get(tag) {
            Some(Value::Doubles(values)) => values.clone(),
            _ => Vec::new(),
        }
    }

    fn text(&self, tag: u16) -> Option<&str> {
        match self.get(tag) {
            Some(Value::Text(text)) => Some(text),
            _ => None,
        }
    }

    /// The GeoKey directory (tag 34735) as plain numbers.
    fn geokeys(&self) -> Vec<u64> {
        self.numbers(34735)
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// Writes a tiny TINITALY-like tile: `width` × `height` cells of 10 m whose top-left corner is
    /// at (`left`, `top`) in UTM 32N, with `value(column, row)` as elevation.
    pub fn write_tile(
        path: &Path,
        (width, height): (usize, usize),
        (left, top): (f64, f64),
        value: impl Fn(usize, usize) -> f32,
    ) {
        let data_offset = 8u32;
        let data_len = (width * height * 4) as u32;
        let mut extra: Vec<u8> = Vec::new(); // out-of-line tag values, after the image data
        let extra_base = data_offset + data_len;
        let mut entries: Vec<[u8; 12]> = Vec::new();
        let mut entry = |tag: u16, kind: u16, count: u32, inline: Option<u32>, payload: &[u8]| {
            let mut bytes = [0u8; 12];
            bytes[0..2].copy_from_slice(&tag.to_le_bytes());
            bytes[2..4].copy_from_slice(&kind.to_le_bytes());
            bytes[4..8].copy_from_slice(&count.to_le_bytes());
            let field = inline.unwrap_or_else(|| {
                let at = extra_base + extra.len() as u32;
                extra.extend_from_slice(payload);
                at
            });
            bytes[8..12].copy_from_slice(&field.to_le_bytes());
            entries.push(bytes);
        };
        let strips: Vec<u8> = (0..height)
            .flat_map(|row| (data_offset + (row * width * 4) as u32).to_le_bytes())
            .collect();
        let doubles = |values: &[f64]| {
            values
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect::<Vec<u8>>()
        };
        let shorts = |values: &[u16]| {
            values
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect::<Vec<u8>>()
        };
        entry(256, 4, 1, Some(width as u32), &[]);
        entry(257, 4, 1, Some(height as u32), &[]);
        entry(258, 3, 1, Some(32), &[]);
        entry(259, 3, 1, Some(1), &[]);
        entry(273, 4, height as u32, None, &strips);
        entry(277, 3, 1, Some(1), &[]);
        entry(278, 4, 1, Some(1), &[]);
        entry(339, 3, 1, Some(3), &[]);
        entry(33550, 12, 3, None, &doubles(&[10.0, 10.0, 0.0]));
        entry(
            33922,
            12,
            6,
            None,
            &doubles(&[0.0, 0.0, 0.0, left, top, 0.0]),
        );
        entry(34735, 3, 8, None, &shorts(&[1, 1, 0, 1, 3072, 0, 1, 32632]));
        entry(42113, 2, 6, None, b"-9999\0");

        let ifd_offset = extra_base + extra.len() as u32;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"II\x2a\x00");
        bytes.extend_from_slice(&ifd_offset.to_le_bytes());
        for row in 0..height {
            for column in 0..width {
                bytes.extend_from_slice(&value(column, row).to_le_bytes());
            }
        }
        bytes.extend_from_slice(&extra);
        bytes.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        for entry in &entries {
            bytes.extend_from_slice(entry);
        }
        bytes.extend_from_slice(&0u32.to_le_bytes());
        std::fs::write(path, bytes).expect("write test tile");
    }

    fn temp_folder(name: &str) -> PathBuf {
        let folder =
            std::env::temp_dir().join(format!("route-catalogue-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).expect("temp folder");
        folder
    }

    #[test]
    fn projects_to_utm_32n() {
        let folder = temp_folder("utm");
        write_tile(&folder.join("t.tif"), (2, 2), (0.0, 0.0), |_, _| 0.0);
        let dem = Dem::open(&folder).expect("open");
        // On the zone's central meridian (9° E) at 45° N: easting 500 km, northing = 0.9996 × the
        // WGS 84 meridian arc to 45° (4,984,944.378 m)
        let (easting, northing) = dem.to_utm((9.0, 45.0)).expect("projects");
        assert!((easting - 500_000.0).abs() < 0.001, "easting {easting}");
        assert!(
            (northing - 4_982_950.400).abs() < 0.01,
            "northing {northing}"
        );
        // East of the meridian: easting grows, and stays in the zone's range
        let (east, _) = dem.to_utm((10.33, 44.03)).expect("projects");
        assert!((600_000.0..620_000.0).contains(&east), "easting {east}");
    }

    #[test]
    fn interpolates_between_cell_centres() {
        let folder = temp_folder("bilinear");
        // Elevation = 100 + 2 × column + 3 × row, a plane: bilinear interpolation must be exact
        write_tile(
            &folder.join("t.tif"),
            (5, 5),
            (600_000.0, 4_900_050.0),
            |c, r| 100.0 + 2.0 * c as f32 + 3.0 * r as f32,
        );
        let dem = Dem::open(&folder).expect("open");
        let tile = &dem.tiles[0];
        // Centre of cell (1, 1) is 15 m right of and 15 m below the corner
        assert_eq!(tile.sample(600_015.0, 4_900_035.0), Some(105.0));
        // Halfway between cells (1, 1) and (2, 2)
        let between = tile.sample(600_020.0, 4_900_030.0).expect("inside");
        assert!((between - 107.5).abs() < 1e-9, "{between}");
        // Outside, or too close to the edge for four neighbours
        assert_eq!(tile.sample(599_000.0, 4_900_000.0), None);
        assert_eq!(tile.sample(600_049.0, 4_900_001.0), None);
    }

    #[test]
    fn missing_data_gives_none() {
        let folder = temp_folder("nodata");
        write_tile(
            &folder.join("t.tif"),
            (3, 3),
            (600_000.0, 4_900_030.0),
            |c, r| {
                if (c, r) == (1, 1) { -9999.0 } else { 50.0 }
            },
        );
        let dem = Dem::open(&folder).expect("open");
        assert_eq!(dem.tiles[0].sample(600_010.0, 4_900_020.0), None);
    }

    #[test]
    fn refuses_other_formats() {
        let folder = temp_folder("refuse");
        std::fs::write(folder.join("bad.tif"), b"MM\x00\x2a not a tinitaly tile").expect("write");
        let Err(error) = Dem::open(&folder) else {
            panic!("a big-endian file must be refused");
        };
        assert!(error.0.contains("little-endian"), "{}", error.0);
        let Err(error) = Dem::open(&temp_folder("empty")) else {
            panic!("an empty folder must be refused");
        };
        assert!(error.0.contains("fetch-dem.sh"), "{}", error.0);
    }
}
