//! The screen size agents see on this computer, picked once at connect from the
//! main display's shape.

/// The sizes an agent's desktop may have, smallest first.
pub const ALLOWED: [(u16, u16); 4] = [(1366, 768), (1440, 900), (1920, 1080), (1920, 1200)];

/// The agent size for a main display of `w_px` by `h_px` pixels. Within 2% of 16:9:
/// 1920x1080 (1366x768 with `small`); within 2% of 16:10: 1920x1200 (1440x900);
/// anything else: the allowed size whose aspect is nearest, ties to the larger. (Both
/// sizes of a shape share its aspect, so "nearest" picks the shape and `small` the size.)
#[allow(dead_code)] // the connect step that probes the display calls it
pub fn pick_size(w_px: u32, h_px: u32, small: bool) -> (u16, u16) {
    const WIDE: f64 = 16.0 / 9.0;
    const TALL: f64 = 16.0 / 10.0;
    // How far a display's aspect may be from 16:9 or 16:10 and still count as it.
    const TOLERANCE: f64 = 0.02;
    let aspect = if h_px == 0 {
        f64::INFINITY
    } else {
        f64::from(w_px) / f64::from(h_px)
    };
    let near = |target: f64| ((aspect - target) / target).abs() <= TOLERANCE;
    let wide = if near(WIDE) {
        true
    } else if near(TALL) {
        false
    } else {
        // The nearest of the two shapes; a tie goes to 16:10, the larger size.
        (aspect - WIDE).abs() < (aspect - TALL).abs()
    };
    match (wide, small) {
        (true, false) => ALLOWED[2],
        (true, true) => ALLOWED[0],
        (false, false) => ALLOWED[3],
        (false, true) => ALLOWED[1],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_size_table() {
        for (w, h, small, want) in [
            (2880, 1800, false, (1920, 1200)),
            (2880, 1800, true, (1440, 900)),
            (3840, 2160, false, (1920, 1080)),
            (3840, 2160, true, (1366, 768)),
            // A notched laptop panel, 16:10.4: still 16:10.
            (3024, 1964, false, (1920, 1200)),
            (3024, 1964, true, (1440, 900)),
            // 21:9 is nearest 16:9.
            (5120, 2160, false, (1920, 1080)),
            (5120, 2160, true, (1366, 768)),
            (1920, 1080, false, (1920, 1080)),
            (1920, 1200, true, (1440, 900)),
        ] {
            assert_eq!(pick_size(w, h, small), want, "{w}x{h} small={small}");
        }
    }

    #[test]
    fn an_odd_shape_takes_the_nearest_aspect() {
        // 4:3 (1.333) is nearest 16:10 (1.6) of the allowed aspects.
        assert_eq!(pick_size(2048, 1536, false), (1920, 1200));
        assert_eq!(pick_size(2048, 1536, true), (1440, 900));
        // A portrait display is nearest the tallest shape too.
        assert_eq!(pick_size(1080, 1920, false), (1920, 1200));
        // A zero height never panics.
        assert!(ALLOWED.contains(&pick_size(1920, 0, false)));
    }
}
