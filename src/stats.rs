use std::{fs::File, io::Read};

use clap::{App, Arg, ArgMatches, SubCommand};
use libflate::gzip::Decoder;

use crate::generator::AllData;

pub const SUBCOMMAND: &str = "stats";

pub fn run(matches: &ArgMatches<'_>) -> Result<(), String> {
    let filename = matches
        .value_of("input")
        .expect("please provide an input file");

    let mut file = File::open(filename).map_err(|e| format!("couldn't open input file: {e}"))?;
    let mut zipped_data = vec![];
    file.read_to_end(&mut zipped_data)
        .map_err(|e| format!("couldn't read input file: {e}"))?;

    let mut decoder = Decoder::new(&zipped_data[..]).map_err(|e| format!("gzip error: {e}"))?;
    let mut data = vec![];
    decoder
        .read_to_end(&mut data)
        .map_err(|e| format!("gzip inflate error: {e}"))?;

    let data: AllData =
        bincode::deserialize(&data[..]).map_err(|e| format!("bincode error: {e}"))?;

    let (water_level, water_level_epsilon) = match data.params.view.coloring {
        crate::generator::params::Coloring::Simple {
            water_level,
            water_level_epsilon,
            ..
        }
        | crate::generator::params::Coloring::Shading {
            water_level,
            water_level_epsilon,
            ..
        } => (water_level, water_level_epsilon),
    };
    let water_level_effective = water_level + water_level_epsilon;

    let mut total_pixels: u64 = 0;
    let mut sky_pixels: u64 = 0;
    let mut terrain_pixels: u64 = 0;
    let mut water_pixels: u64 = 0;
    let mut land_pixels: u64 = 0;

    let mut water_above_eye_level: u64 = 0;
    let mut terrain_above_eye_level: u64 = 0;

    let mut bottom_rows_terrain: u64 = 0;
    let mut bottom_rows_water: u64 = 0;
    let mut bottom_rows_elev_min: f64 = f64::INFINITY;
    let mut bottom_rows_elev_max: f64 = f64::NEG_INFINITY;
    let mut bottom_rows_elev_le_1m: u64 = 0;
    let mut bottom_rows_elev_le_5m: u64 = 0;
    let mut bottom_rows_elev_le_20m: u64 = 0;
    let bottom_rows = 80usize;

    let height = data.result.len();

    for (y, row) in data.result.iter().enumerate() {
        for px in row {
            total_pixels += 1;

            let above_eye = px.elevation_angle > 0.0;

            let Some(first) = px.trace_points.first() else {
                sky_pixels += 1;
                continue;
            };

            match first.color {
                crate::generator::PixelColor::Terrain(_) => {
                    terrain_pixels += 1;
                    let is_water = first.elevation <= water_level_effective;
                    if is_water {
                        water_pixels += 1;
                        if above_eye {
                            water_above_eye_level += 1;
                        }
                    } else {
                        land_pixels += 1;
                    }

                    if above_eye {
                        terrain_above_eye_level += 1;
                    }

                    if height.saturating_sub(y) <= bottom_rows {
                        bottom_rows_terrain += 1;
                        bottom_rows_elev_min = bottom_rows_elev_min.min(first.elevation);
                        bottom_rows_elev_max = bottom_rows_elev_max.max(first.elevation);
                        if is_water {
                            bottom_rows_water += 1;
                        }
                        if first.elevation <= 1.0 {
                            bottom_rows_elev_le_1m += 1;
                        }
                        if first.elevation <= 5.0 {
                            bottom_rows_elev_le_5m += 1;
                        }
                        if first.elevation <= 20.0 {
                            bottom_rows_elev_le_20m += 1;
                        }
                    }
                }
                crate::generator::PixelColor::Rgba(_) => {
                    // Not counted as terrain/water/land.
                }
            }
        }
    }

    println!("pixels_total\t{}", total_pixels);
    println!("pixels_sky\t{}", sky_pixels);
    println!("pixels_terrain\t{}", terrain_pixels);
    println!("pixels_water\t{}", water_pixels);
    println!("pixels_land\t{}", land_pixels);
    println!("water_level_m\t{}", water_level);
    println!("water_level_effective_m\t{}", water_level_effective);
    println!("terrain_above_eye\t{}", terrain_above_eye_level);
    println!("water_above_eye\t{}", water_above_eye_level);

    if bottom_rows_terrain > 0 {
        let water_ratio = bottom_rows_water as f64 / bottom_rows_terrain as f64;
        println!("bottom_rows\t{}", bottom_rows);
        println!("bottom_rows_terrain\t{}", bottom_rows_terrain);
        println!("bottom_rows_water\t{}", bottom_rows_water);
        println!("bottom_rows_water_ratio\t{:.6}", water_ratio);
        println!("bottom_rows_first_hit_elev_min_m\t{:.3}", bottom_rows_elev_min);
        println!("bottom_rows_first_hit_elev_max_m\t{:.3}", bottom_rows_elev_max);
        println!("bottom_rows_first_hit_elev_le_1m\t{}", bottom_rows_elev_le_1m);
        println!("bottom_rows_first_hit_elev_le_5m\t{}", bottom_rows_elev_le_5m);
        println!("bottom_rows_first_hit_elev_le_20m\t{}", bottom_rows_elev_le_20m);
    }

    Ok(())
}

pub fn subcommand_def() -> App<'static, 'static> {
    SubCommand::with_name(SUBCOMMAND)
        .about("Print basic stats from a metadata .dat file")
        .arg(
            Arg::with_name("input")
                .help("Path to a metadata file produced by gen (--output-meta)")
                .required(true)
                .index(1),
        )
}
