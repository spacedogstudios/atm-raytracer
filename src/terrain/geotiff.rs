use std::{fs::File, io::BufReader, path::Path, str::FromStr};

use lazy_static::lazy_static;
use regex::Regex;
use tiff::decoder::{Decoder, DecodingResult};

use super::Tile;

pub struct GeoTiffWrapper {
    min_lat: f64,
    min_lon: f64,
    width: usize,
    height: usize,
    data: RasterData,
}

enum RasterData {
    I16(Vec<i16>),
    U16(Vec<u16>),
    I32(Vec<i32>),
    U32(Vec<u32>),
    F32(Vec<f32>),
}

enum ReducedChunk {
    I16(Vec<i16>),
    U16(Vec<u16>),
    I32(Vec<i32>),
    U32(Vec<u32>),
    F32(Vec<f32>),
}

impl GeoTiffWrapper {
    pub fn coords_from_name(name: &Path) -> Option<(i16, i16)> {
        lazy_static! {
            // Accept common DEM naming schemes (case-insensitive, separators allowed), e.g.:
            // - "N34W119.tif"
            // - "n34_w119_20250916.tif"
            static ref RE: Regex =
                Regex::new("(?i)(N|S)\\s*(\\d{1,2})\\D*(E|W)\\s*(\\d{1,3})").unwrap();
        }
        let file_name = name.file_name()?.to_str()?;
        let cap = RE.captures_iter(file_name).next()?;
        let mut lat = i16::from_str(&cap[2]).ok()?;
        if cap[1].eq_ignore_ascii_case("S") {
            lat = -lat;
        }
        let mut lon = i16::from_str(&cap[4]).ok()?;
        if cap[3].eq_ignore_ascii_case("W") {
            lon = -lon;
        }
        Some((lat, lon))
    }

    fn reduce_channels<T: Copy>(values: Vec<T>, pixel_count: usize) -> Result<Vec<T>, String> {
        if values.len() == pixel_count {
            return Ok(values);
        }
        if !values.len().is_multiple_of(pixel_count) {
            return Err(format!(
                "Decoded TIFF had {} samples, expected a multiple of {} pixels",
                values.len(),
                pixel_count
            ));
        }
        let channels = values.len() / pixel_count;
        if channels == 0 {
            return Err("Decoded TIFF had 0 channels".to_string());
        }

        // Keep only the first sample per pixel.
        let mut out = Vec::with_capacity(pixel_count);
        for chunk in values.chunks_exact(channels) {
            out.push(chunk[0]);
        }
        Ok(out)
    }

    fn decode_to_reduced_chunk(
        decoded: DecodingResult,
        pixel_count: usize,
    ) -> Result<ReducedChunk, String> {
        match decoded {
            DecodingResult::I16(v) => Ok(ReducedChunk::I16(Self::reduce_channels(v, pixel_count)?)),
            DecodingResult::U16(v) => Ok(ReducedChunk::U16(Self::reduce_channels(v, pixel_count)?)),
            DecodingResult::I32(v) => Ok(ReducedChunk::I32(Self::reduce_channels(v, pixel_count)?)),
            DecodingResult::U32(v) => Ok(ReducedChunk::U32(Self::reduce_channels(v, pixel_count)?)),
            DecodingResult::F32(v) => Ok(ReducedChunk::F32(Self::reduce_channels(v, pixel_count)?)),
            DecodingResult::F64(v) => {
                let v: Vec<f32> = v.into_iter().map(|x| x as f32).collect();
                Ok(ReducedChunk::F32(Self::reduce_channels(v, pixel_count)?))
            }
            DecodingResult::I8(v) => {
                let v: Vec<i16> = v.into_iter().map(|x| x as i16).collect();
                Ok(ReducedChunk::I16(Self::reduce_channels(v, pixel_count)?))
            }
            DecodingResult::U8(v) => {
                let v: Vec<u16> = v.into_iter().map(|x| x as u16).collect();
                Ok(ReducedChunk::U16(Self::reduce_channels(v, pixel_count)?))
            }
            DecodingResult::I64(v) => {
                let v: Vec<i32> = v.into_iter().map(|x| x as i32).collect();
                Ok(ReducedChunk::I32(Self::reduce_channels(v, pixel_count)?))
            }
            DecodingResult::U64(v) => {
                let v: Vec<u32> = v.into_iter().map(|x| x as u32).collect();
                Ok(ReducedChunk::U32(Self::reduce_channels(v, pixel_count)?))
            }
        }
    }

    fn from_path_result(name: &Path) -> Result<Self, String> {
        let (lat, lon) = Self::coords_from_name(name)
            .ok_or_else(|| format!("failed parsing lat/lon from filename {:?}", name))?;

        let file =
            File::open(name).map_err(|e| format!("failed opening GeoTIFF {:?}: {e}", name))?;
        let mut decoder = Decoder::new(BufReader::new(file))
            .map_err(|e| format!("failed initializing TIFF decoder for {:?}: {e}", name))?;

        let (width, height) = decoder
            .dimensions()
            .map_err(|e| format!("failed reading TIFF dimensions for {:?}: {e}", name))?;
        let width = width as usize;
        let height = height as usize;
        if width == 0 || height == 0 {
            return Err(format!(
                "TIFF {:?} had invalid dimensions {}x{}",
                name, width, height
            ));
        }

        let pixel_count = width
            .checked_mul(height)
            .ok_or_else(|| format!("TIFF {:?} dimensions overflow", name))?;

        let (chunk_width, chunk_height) = decoder.chunk_dimensions();
        let chunk_width = chunk_width as usize;
        let chunk_height = chunk_height as usize;
        if chunk_width == 0 || chunk_height == 0 {
            return Err(format!(
                "TIFF {:?} had invalid chunk dimensions {}x{}",
                name, chunk_width, chunk_height
            ));
        }
        let chunks_across = width.div_ceil(chunk_width);

        let chunk_count = decoder
            .tile_count()
            .or_else(|_| decoder.strip_count())
            .map_err(|e| format!("failed reading TIFF chunk count for {:?}: {e}", name))?;

        println!(
            "Decoding GeoTIFF tile {:?} ({}x{}, {} chunks)",
            name, width, height, chunk_count
        );

        let (first_data_width, first_data_height) = decoder.chunk_data_dimensions(0);
        let first_data_width = first_data_width as usize;
        let first_data_height = first_data_height as usize;
        let first_pixel_count = first_data_width
            .checked_mul(first_data_height)
            .ok_or_else(|| format!("TIFF {:?} chunk dimensions overflow", name))?;
        let first_decoded = decoder
            .read_chunk(0)
            .map_err(|e| format!("failed decoding TIFF chunk 0 for {:?}: {e}", name))?;
        let first_chunk = Self::decode_to_reduced_chunk(first_decoded, first_pixel_count)
            .map_err(|e| format!("failed converting TIFF chunk 0 for {:?}: {e}", name))?;

        let mut data = match &first_chunk {
            ReducedChunk::I16(_) => RasterData::I16(vec![0i16; pixel_count]),
            ReducedChunk::U16(_) => RasterData::U16(vec![0u16; pixel_count]),
            ReducedChunk::I32(_) => RasterData::I32(vec![0i32; pixel_count]),
            ReducedChunk::U32(_) => RasterData::U32(vec![0u32; pixel_count]),
            ReducedChunk::F32(_) => RasterData::F32(vec![0.0f32; pixel_count]),
        };

        // Helper: copy a reduced chunk into the destination raster.
        #[allow(clippy::too_many_arguments)]
        fn blit<T: Copy>(
            dst: &mut [T],
            dst_width: usize,
            dst_height: usize,
            src: &[T],
            src_width: usize,
            src_height: usize,
            start_x: usize,
            start_y: usize,
        ) {
            for y in 0..src_height {
                let dst_y = start_y + y;
                if dst_y >= dst_height {
                    break;
                }
                let dst_row = dst_y * dst_width;
                let src_row = y * src_width;
                let max_x = (start_x + src_width).min(dst_width);
                for x in start_x..max_x {
                    dst[dst_row + x] = src[src_row + (x - start_x)];
                }
            }
        }

        // Blit chunk 0 (already decoded).
        {
            let chunk_col = 0usize;
            let chunk_row = 0usize;
            let start_x = chunk_col * chunk_width;
            let start_y = chunk_row * chunk_height;
            match (&mut data, first_chunk) {
                (RasterData::I16(dst), ReducedChunk::I16(src)) => blit(
                    dst,
                    width,
                    height,
                    &src,
                    first_data_width,
                    first_data_height,
                    start_x,
                    start_y,
                ),
                (RasterData::U16(dst), ReducedChunk::U16(src)) => blit(
                    dst,
                    width,
                    height,
                    &src,
                    first_data_width,
                    first_data_height,
                    start_x,
                    start_y,
                ),
                (RasterData::I32(dst), ReducedChunk::I32(src)) => blit(
                    dst,
                    width,
                    height,
                    &src,
                    first_data_width,
                    first_data_height,
                    start_x,
                    start_y,
                ),
                (RasterData::U32(dst), ReducedChunk::U32(src)) => blit(
                    dst,
                    width,
                    height,
                    &src,
                    first_data_width,
                    first_data_height,
                    start_x,
                    start_y,
                ),
                (RasterData::F32(dst), ReducedChunk::F32(src)) => blit(
                    dst,
                    width,
                    height,
                    &src,
                    first_data_width,
                    first_data_height,
                    start_x,
                    start_y,
                ),
                _ => {
                    return Err(format!(
                        "TIFF {:?} chunk 0 decoded to an unexpected type",
                        name
                    ));
                }
            }
        }

        // Decode remaining chunks sequentially using the existing decoder.
        // This is intentionally simple and portable (no unsafe + no machine-specific tuning).
        for chunk_index in 1..chunk_count {
            let (data_width, data_height) = decoder.chunk_data_dimensions(chunk_index);
            let data_width = data_width as usize;
            let data_height = data_height as usize;
            let chunk_pixel_count = data_width
                .checked_mul(data_height)
                .ok_or_else(|| format!("TIFF {:?} chunk dimensions overflow", name))?;

            let decoded = decoder.read_chunk(chunk_index).map_err(|e| {
                format!(
                    "failed decoding TIFF chunk {chunk_index} for {:?}: {e}",
                    name
                )
            })?;
            let reduced =
                Self::decode_to_reduced_chunk(decoded, chunk_pixel_count).map_err(|e| {
                    format!(
                        "failed converting TIFF chunk {chunk_index} for {:?}: {e}",
                        name
                    )
                })?;

            let chunk_col = chunk_index as usize % chunks_across;
            let chunk_row = chunk_index as usize / chunks_across;
            let start_x = chunk_col * chunk_width;
            let start_y = chunk_row * chunk_height;

            match (&mut data, reduced) {
                (RasterData::I16(dst), ReducedChunk::I16(src)) => blit(
                    dst,
                    width,
                    height,
                    &src,
                    data_width,
                    data_height,
                    start_x,
                    start_y,
                ),
                (RasterData::U16(dst), ReducedChunk::U16(src)) => blit(
                    dst,
                    width,
                    height,
                    &src,
                    data_width,
                    data_height,
                    start_x,
                    start_y,
                ),
                (RasterData::I32(dst), ReducedChunk::I32(src)) => blit(
                    dst,
                    width,
                    height,
                    &src,
                    data_width,
                    data_height,
                    start_x,
                    start_y,
                ),
                (RasterData::U32(dst), ReducedChunk::U32(src)) => blit(
                    dst,
                    width,
                    height,
                    &src,
                    data_width,
                    data_height,
                    start_x,
                    start_y,
                ),
                (RasterData::F32(dst), ReducedChunk::F32(src)) => blit(
                    dst,
                    width,
                    height,
                    &src,
                    data_width,
                    data_height,
                    start_x,
                    start_y,
                ),
                _ => {
                    return Err(format!(
                        "TIFF {:?} chunk {chunk_index} decoded to an inconsistent type",
                        name
                    ));
                }
            }
        }

        Ok(Self {
            min_lat: lat as f64,
            min_lon: lon as f64,
            width,
            height,
            data,
        })
    }

    pub fn from_path(name: &Path) -> Option<Self> {
        match Self::from_path_result(name) {
            Ok(v) => Some(v),
            Err(msg) => {
                eprintln!("GeoTIFF load failed: {msg}");
                None
            }
        }
    }

    fn pixel(&self, x: usize, y: usize) -> Option<f64> {
        if x >= self.width || y >= self.height {
            return None;
        }

        let idx = y * self.width + x;
        match &self.data {
            RasterData::I16(v) => Some(v[idx] as f64),
            RasterData::U16(v) => Some(v[idx] as f64),
            RasterData::I32(v) => Some(v[idx] as f64),
            RasterData::U32(v) => Some(v[idx] as f64),
            RasterData::F32(v) => Some(v[idx] as f64),
        }
    }
}

impl Tile for GeoTiffWrapper {
    fn min_latitude(&self) -> f64 {
        self.min_lat
    }

    fn max_latitude(&self) -> f64 {
        self.min_lat + 1.0
    }

    fn min_longitude(&self) -> f64 {
        self.min_lon
    }

    fn max_longitude(&self) -> f64 {
        self.min_lon + 1.0
    }

    fn get_elev(&self, lat: f64, lon: f64) -> Option<f64> {
        if lat < self.min_latitude()
            || lat > self.max_latitude()
            || lon < self.min_longitude()
            || lon > self.max_longitude()
        {
            return None;
        }

        // Map lat/lon within a 1x1 degree tile to raster coordinates.
        // Assume row 0 is the northern edge (common for GeoTIFF DEMs).
        let frac_lat = (lat - self.min_lat) / (self.max_latitude() - self.min_latitude());
        let frac_lon = (lon - self.min_lon) / (self.max_longitude() - self.min_longitude());

        let x = frac_lon * (self.width as f64 - 1.0);
        let y = (1.0 - frac_lat) * (self.height as f64 - 1.0);

        let mut x0 = x.floor() as usize;
        let mut y0 = y.floor() as usize;
        let mut xf = x - x0 as f64;
        let mut yf = y - y0 as f64;

        // handle the edge case of max lat/lon
        if x0 + 1 >= self.width {
            x0 = self.width - 2;
            xf = 1.0;
        }
        if y0 + 1 >= self.height {
            y0 = self.height - 2;
            yf = 1.0;
        }

        let elev00 = self.pixel(x0, y0)?;
        let elev10 = self.pixel(x0 + 1, y0)?;
        let elev01 = self.pixel(x0, y0 + 1)?;
        let elev11 = self.pixel(x0 + 1, y0 + 1)?;

        Some(
            elev00 * (1.0 - xf) * (1.0 - yf)
                + elev10 * xf * (1.0 - yf)
                + elev01 * (1.0 - xf) * yf
                + elev11 * xf * yf,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::GeoTiffWrapper;
    use std::path::Path;

    #[test]
    fn coords_from_name_parses_lowercase_with_separators() {
        let p = Path::new("terrain/n34_w119_20250916.tif");
        assert_eq!(GeoTiffWrapper::coords_from_name(p), Some((34, -119)));
    }

    #[test]
    fn coords_from_name_parses_compact_uppercase() {
        let p = Path::new("N34W119.tif");
        assert_eq!(GeoTiffWrapper::coords_from_name(p), Some((34, -119)));
    }
}
