use std::{
    collections::{hash_map::Entry, HashMap},
    env,
};

use crate::{
    generator::{
        params::{Params, Tick, TickLike, VerticalTick},
        ResultPixel,
    },
    terrain::Terrain,
    utils::{rgb_to_vec3, vec3_to_rgb},
};

use atm_refraction::EarthShape;
use image::{ImageBuffer, Pixel, Rgb};
use imageproc::drawing::{draw_line_segment_mut, draw_text_mut};
use rusttype::{Font, Scale};

static FONT: &[u8] = include_bytes!("DejaVuSans.ttf");

struct DrawTick {
    size: u32,
    angle: String,
    labelled: bool,
}

fn diff_azimuth(az1: f64, az2: f64) -> f64 {
    let diff = az1 - az2;
    if diff < -180.0 {
        diff + 360.0
    } else if diff > 180.0 {
        diff - 360.0
    } else {
        diff
    }
}

fn azimuth_to_x(azimuth: f64, pixels: &[Vec<ResultPixel>]) -> Option<u32> {
    let candidate = pixels[0]
        .iter()
        .enumerate()
        .min_by(|(_, pixel1), (_, pixel2)| {
            diff_azimuth(azimuth, pixel1.azimuth)
                .abs()
                .partial_cmp(&diff_azimuth(azimuth, pixel2.azimuth).abs())
                .unwrap()
        })
        .map(|(idx, _)| idx as u32)
        .unwrap();
    let neighboring_idx = if candidate == 0 { 1 } else { candidate - 1 };
    let diff_per_pixel = diff_azimuth(
        pixels[0][candidate as usize].azimuth,
        pixels[0][neighboring_idx as usize].azimuth,
    )
    .abs();
    (diff_azimuth(pixels[0][candidate as usize].azimuth, azimuth).abs() < diff_per_pixel * 1.5)
        .then_some(candidate)
}

fn elevation_to_y(elevation: f64, pixels: &[Vec<ResultPixel>]) -> Option<u32> {
    let candidate = pixels
        .iter()
        .map(|pixels_row| &pixels_row[0])
        .enumerate()
        .min_by(|(_, pixel1), (_, pixel2)| {
            (elevation - pixel1.elevation_angle)
                .abs()
                .partial_cmp(&(elevation - pixel2.elevation_angle).abs())
                .unwrap()
        })
        .map(|(idx, _)| idx as u32)
        .unwrap();
    let neighboring_idx = if candidate == 0 { 1 } else { candidate - 1 };
    let diff_per_pixel = (pixels[candidate as usize][0].elevation_angle
        - pixels[neighboring_idx as usize][0].elevation_angle)
        .abs();
    ((pixels[candidate as usize][0].elevation_angle - elevation).abs() < diff_per_pixel * 1.5)
        .then_some(candidate)
}

fn into_draw_ticks(
    tick: &Tick,
    params: &Params,
    pixels: &[Vec<ResultPixel>],
    decimals: usize,
) -> Vec<(u32, DrawTick)> {
    match *tick {
        Tick::Single {
            azimuth,
            size,
            labelled,
        } => {
            if let Some(x) = azimuth_to_x(azimuth, pixels) {
                vec![(
                    x,
                    DrawTick {
                        angle: format!("{:.1$}", azimuth, decimals),
                        size,
                        labelled,
                    },
                )]
            } else {
                vec![]
            }
        }
        Tick::Multiple {
            bias,
            step,
            size,
            labelled,
        } => {
            let min_az = params.view.frame.direction - params.view.frame.fov / 2.0;
            let max_az = params.view.frame.direction + params.view.frame.fov / 2.0;
            let mut current_az = ((min_az - bias) / step).ceil() * step + bias;
            let mut result = Vec::new();
            while current_az < max_az {
                let azimuth = if current_az < 0.0 {
                    current_az + 360.0
                } else if current_az >= 360.0 {
                    current_az - 360.0
                } else {
                    current_az
                };
                if let Some(x) = azimuth_to_x(current_az, pixels) {
                    result.push((
                        x,
                        DrawTick {
                            size,
                            labelled,
                            angle: format!("{:.1$}", azimuth, decimals),
                        },
                    ));
                }
                current_az += step;
            }
            result
        }
    }
}

fn into_draw_ticks_vertical(
    tick: &VerticalTick,
    params: &Params,
    pixels: &[Vec<ResultPixel>],
    decimals: usize,
) -> Vec<(u32, DrawTick)> {
    match *tick {
        VerticalTick::Single {
            elevation,
            size,
            labelled,
        } => {
            if let Some(y) = elevation_to_y(elevation, pixels) {
                vec![(
                    y,
                    DrawTick {
                        angle: format!("{:.1$}", elevation, decimals),
                        size,
                        labelled,
                    },
                )]
            } else {
                vec![]
            }
        }
        VerticalTick::Multiple {
            bias,
            step,
            size,
            labelled,
        } => {
            let aspect = params.output.height as f64 / params.output.width as f64;
            let min_elev = params.view.frame.tilt - params.view.frame.fov * aspect / 2.0;
            let max_elev = params.view.frame.tilt + params.view.frame.fov * aspect / 2.0;
            let mut current_elev = ((min_elev - bias) / step).ceil() * step + bias;
            let mut result = Vec::new();
            while current_elev < max_elev {
                let elevation = if current_elev < -90.0 {
                    -180.0 - current_elev
                } else if current_elev > 90.0 {
                    180.0 - current_elev
                } else {
                    current_elev
                };
                if let Some(y) = elevation_to_y(elevation, pixels) {
                    result.push((
                        y,
                        DrawTick {
                            size,
                            labelled,
                            angle: format!("{:.1$}", elevation, decimals),
                        },
                    ));
                }
                current_elev += step;
            }
            result
        }
    }
}

struct TicksToDraw {
    horizontal: HashMap<u32, DrawTick>,
    vertical: HashMap<u32, DrawTick>,
}

fn num_decimals(x: f64) -> usize {
    for i in 0..10 {
        let mul_x = x * 10.0_f64.powi(i as i32);
        if (mul_x.round() - mul_x).abs() < 0.001 {
            return i;
        }
    }
    10
}

fn round_decimals<T: TickLike>(ticks: &[T]) -> usize {
    ticks
        .iter()
        .filter(|tick| tick.labelled())
        .map(|tick| num_decimals(tick.angle()))
        .max()
        .unwrap_or(0)
}

fn gen_ticks(params: &Params, pixels: &[Vec<ResultPixel>]) -> TicksToDraw {
    let mut horizontal = HashMap::new();
    let mut vertical = HashMap::new();

    let horizontal_decimals = round_decimals(&params.output.ticks);
    let vertical_decimals = round_decimals(&params.output.vertical_ticks);

    for tick in &params.output.ticks {
        let new_ticks = into_draw_ticks(tick, params, pixels, horizontal_decimals);
        for (x, tick) in new_ticks {
            match horizontal.entry(x) {
                Entry::Vacant(v) => {
                    v.insert(tick);
                }
                Entry::Occupied(mut o) => {
                    if o.get().size < tick.size {
                        o.insert(tick);
                    }
                }
            }
        }
    }
    for tick in &params.output.vertical_ticks {
        let new_ticks = into_draw_ticks_vertical(tick, params, pixels, vertical_decimals);
        for (y, tick) in new_ticks {
            match vertical.entry(y) {
                Entry::Vacant(v) => {
                    v.insert(tick);
                }
                Entry::Occupied(mut o) => {
                    if o.get().size < tick.size {
                        o.insert(tick);
                    }
                }
            }
        }
    }
    TicksToDraw {
        horizontal,
        vertical,
    }
}

fn draw_ticks(
    img: &mut ImageBuffer<Rgb<u8>, Vec<<Rgb<u8> as Pixel>::Subpixel>>,
    params: &Params,
    pixels: &[Vec<ResultPixel>],
) {
    let font = Font::try_from_bytes(FONT).unwrap();
    let height = 15.0;
    let scale = Scale {
        x: height,
        y: height,
    };
    let TicksToDraw {
        horizontal,
        vertical,
    } = gen_ticks(params, pixels);
    for (x, tick) in horizontal {
        draw_line_segment_mut(
            img,
            (x as f32, 0.0),
            (x as f32, tick.size as f32),
            Rgb([255, 255, 255]),
        );
        if tick.labelled {
            draw_text_mut(
                img,
                Rgb([255, 255, 255]),
                x as i32 - 8,
                tick.size as i32 + 5,
                scale,
                &font,
                &tick.angle.to_string(),
            );
        }
    }
    for (y, tick) in vertical {
        draw_line_segment_mut(
            img,
            (0.0, y as f32),
            (tick.size as f32, y as f32),
            Rgb([255, 255, 255]),
        );
        if tick.labelled {
            draw_text_mut(
                img,
                Rgb([255, 255, 255]),
                tick.size as i32 + 5,
                y as i32 - 7,
                scale,
                &font,
                &tick.angle.to_string(),
            );
        }
    }
}

fn find_elev(pixels: &[Vec<ResultPixel>], column: u32, elev: f64) -> Option<u32> {
    let mut closest_elev = f64::INFINITY;
    let mut closest_elev_idx = 0;
    for (y, row) in pixels.iter().enumerate() {
        if (row[column as usize].elevation_angle - elev).abs() < (closest_elev - elev).abs() {
            closest_elev = row[column as usize].elevation_angle;
            closest_elev_idx = y;
        }
    }
    let neighbor = if closest_elev_idx == 0 {
        1
    } else {
        closest_elev_idx - 1
    };
    let neighbor_elev = pixels[neighbor][column as usize].elevation_angle;

    ((closest_elev - elev).abs() < (neighbor_elev - closest_elev).abs() * 1.5)
        .then_some(closest_elev_idx as u32)
}

fn draw_const_elev(
    img: &mut ImageBuffer<Rgb<u8>, Vec<<Rgb<u8> as Pixel>::Subpixel>>,
    params: &Params,
    pixels: &[Vec<ResultPixel>],
    elev: f64,
    color: [u8; 3],
) {
    let mut maybe_y_old = find_elev(pixels, 0, elev);
    for x in 1..params.output.width {
        let maybe_y_new = find_elev(pixels, x as u32, elev);
        if let (Some(y_old), Some(y_new)) = (maybe_y_old, maybe_y_new) {
            draw_line_segment_mut(
                img,
                ((x - 1) as f32, y_old as f32),
                (x as f32, y_new as f32),
                Rgb(color),
            );
        }
        maybe_y_old = maybe_y_new;
    }
}

fn fog(fog_dist: f64, pixel_dist: f64, color: Rgb<u8>) -> Rgb<u8> {
    let fog_coeff = 1.0 - (-pixel_dist / fog_dist).exp();
    let fog_color = Rgb([160u8, 160, 160]);
    let mut new_color = Rgb([0u8; 3]);
    for i in 0..3 {
        new_color.0[i] =
            (color.0[i] as f64 * (1.0 - fog_coeff) + fog_color.0[i] as f64 * fog_coeff) as u8;
    }
    new_color
}

fn add(rgb1: Rgb<u8>, rgb2: Rgb<u8>, a: f64) -> Rgb<u8> {
    let color1 = rgb_to_vec3(rgb1);
    let color2 = rgb_to_vec3(rgb2);
    let result = color1 + color2 * a;
    vec3_to_rgb(result)
}

pub fn draw_image(pixels: &[Vec<ResultPixel>], params: &Params) -> ImageBuffer<Rgb<u8>, Vec<u8>> {
    let mut img = ImageBuffer::new(params.output.width as u32, params.output.height as u32);
    let coloring = params.view.coloring.coloring_method();

    // Some DEMs have coastline/interpolation artifacts where sea pixels end up slightly above
    // sea level (e.g. +0.89m everywhere). We treat elevations within WATER_LEVEL_EPS_M above
    // `water_level` as water *unless* they touch definite land (> water_level + eps).
    let (water_level, water_level_epsilon) = match params.view.coloring {
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

    let w = params.output.width as usize;
    let h = params.output.height as usize;
    let mut first_terrain_elev: Vec<Vec<Option<f64>>> = vec![vec![None; w]; h];
    for y in 0..h {
        for x in 0..w {
            let Some(tp) = pixels[y][x].trace_points.first() else {
                continue;
            };
            if matches!(tp.color, crate::generator::PixelColor::Terrain(_)) {
                first_terrain_elev[y][x] = Some(tp.elevation);
            }
        }
    }

    let mut strict_land: Vec<Vec<bool>> = vec![vec![false; w]; h];
    let mut near_band: Vec<Vec<bool>> = vec![vec![false; w]; h];
    for y in 0..h {
        for x in 0..w {
            let Some(e) = first_terrain_elev[y][x] else {
                continue;
            };
            if e > water_level_effective {
                strict_land[y][x] = true;
            } else if water_level_epsilon > 0.0 && e > water_level && e <= water_level_effective {
                near_band[y][x] = true;
            }
        }
    }

    let mut near_band_should_be_land: Vec<Vec<bool>> = vec![vec![false; w]; h];
    for y in 0..h {
        for x in 0..w {
            if !near_band[y][x] {
                continue;
            }
            let y0 = y.saturating_sub(1);
            let y1 = (y + 1).min(h - 1);
            let x0 = x.saturating_sub(1);
            let x1 = (x + 1).min(w - 1);
            let mut touches_land = false;
            'nb: for yy in y0..=y1 {
                for xx in x0..=x1 {
                    if yy == y && xx == x {
                        continue;
                    }
                    if strict_land[yy][xx] {
                        touches_land = true;
                        break 'nb;
                    }
                }
            }
            near_band_should_be_land[y][x] = touches_land;
        }
    }

    let def_color = if params.view.fog_distance.is_some() {
        coloring.fog_color()
        //Rgb([160, 160, 160])
    } else {
        coloring.sky_color()
        //Rgb([28, 28, 28])
    };
    for (x, y, px) in img.enumerate_pixels_mut() {
        let mut result = Rgb([0, 0, 0]);
        let mut accum_neg_alpha = 1.0;

        for (idx, pixel) in pixels[y as usize][x as usize].trace_points.iter().enumerate() {
            // Apply near-water shoreline rule to the *first* terrain hit only.
            let mut pixel_for_coloring = *pixel;
            if idx == 0
                && near_band_should_be_land[y as usize][x as usize]
                && matches!(pixel_for_coloring.color, crate::generator::PixelColor::Terrain(_))
                && pixel_for_coloring.elevation > water_level
                && pixel_for_coloring.elevation <= water_level_effective
            {
                // Force it over the threshold so coloring treats it as land.
                pixel_for_coloring.elevation = water_level_effective + 1e-6;
            }

            let color1 = coloring.color_for_pixel(&pixel_for_coloring);
            let color2 = if let Some(fog_dist) = params.view.fog_distance {
                fog(fog_dist, pixel_for_coloring.path_length, color1)
            } else {
                color1
            };
            result = add(result, color2, accum_neg_alpha * pixel_for_coloring.color.alpha());
            accum_neg_alpha *= 1.0 - pixel_for_coloring.color.alpha();
        }

        *px = add(result, def_color, accum_neg_alpha);
    }

    img
}

pub fn output_image(pixels: &[Vec<ResultPixel>], params: &Params, terrain: &Terrain) {
    let mut img = draw_image(pixels, params);

    if params.output.show_ticks
        && (!params.output.ticks.is_empty() || !params.output.vertical_ticks.is_empty())
    {
        draw_ticks(&mut img, params, pixels);
    }
    if params.output.show_flat_horizon
        && matches!(params.env.shape, EarthShape::Flat)
        && !params.straight_rays
    {
        let observer_alt = params.view.position.abs_altitude(terrain);
        let n_at_observer_height = params.env.n(observer_alt);
        let elev = (1.0 / n_at_observer_height).acos().to_degrees();
        draw_const_elev(&mut img, params, pixels, elev, [0, 128, 255]);
    }
    if params.output.show_eye_level {
        draw_const_elev(&mut img, params, pixels, 0.0, [255, 128, 255]);
    }

    let mut output_file = env::current_dir().unwrap();
    output_file.push(&params.output.file);

    img.save(output_file).unwrap();
}

#[cfg(test)]
mod tests {
    use super::num_decimals;

    #[test]
    fn test_decimals() {
        assert_eq!(num_decimals(0.0), 0);
        assert_eq!(num_decimals(1.0), 0);
        assert_eq!(num_decimals(15.0), 0);
        assert_eq!(num_decimals(183.0), 0);
        assert_eq!(num_decimals(0.1), 1);
        assert_eq!(num_decimals(0.3), 1);
        assert_eq!(num_decimals(0.9), 1);
        assert_eq!(num_decimals(1.8), 1);
        assert_eq!(num_decimals(12.6), 1);
        assert_eq!(num_decimals(133.5), 1);
        assert_eq!(num_decimals(0.25), 2);
        assert_eq!(num_decimals(33.99), 2);
        assert_eq!(num_decimals(33.01), 2);
        assert_eq!(num_decimals(133.01002), 5);
    }
}
