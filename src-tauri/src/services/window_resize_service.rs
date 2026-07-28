use tauri::{LogicalSize, WebviewWindow};

use crate::error::AppError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowLayout {
    pub width: i32,
    pub height: i32,
}

pub const MAIN_WINDOW_LAYOUT: WindowLayout = WindowLayout {
    width: 892,
    height: 496,
};
pub const UPDATE_WINDOW_LAYOUT: WindowLayout = WindowLayout {
    width: 788,
    height: 272,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SizingEdge {
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ResizeRect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

impl ResizeRect {
    const fn new(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }

    fn width(self) -> i32 {
        self.right.saturating_sub(self.left)
    }

    fn height(self) -> i32 {
        self.bottom.saturating_sub(self.top)
    }
}

#[derive(Clone, Copy)]
enum DimensionDriver {
    Width,
    Height,
}

fn rounded_ratio(value: i32, numerator: i32, denominator: i32) -> i32 {
    let value = i64::from(value.max(1));
    let numerator = i64::from(numerator.max(1));
    let denominator = i64::from(denominator.max(1));
    let rounded = value
        .saturating_mul(numerator)
        .saturating_add(denominator / 2)
        / denominator;
    i32::try_from(rounded).unwrap_or(i32::MAX)
}

fn dimension_driver(
    edge: SizingEdge,
    proposed: ResizeRect,
    current: ResizeRect,
    layout: WindowLayout,
) -> DimensionDriver {
    match edge {
        SizingEdge::Left | SizingEdge::Right => DimensionDriver::Width,
        SizingEdge::Top | SizingEdge::Bottom => DimensionDriver::Height,
        SizingEdge::TopLeft
        | SizingEdge::TopRight
        | SizingEdge::BottomLeft
        | SizingEdge::BottomRight => {
            let width_delta = i64::from((proposed.width() - current.width()).abs());
            let height_delta = i64::from((proposed.height() - current.height()).abs());
            if width_delta.saturating_mul(i64::from(layout.height))
                >= height_delta.saturating_mul(i64::from(layout.width))
            {
                DimensionDriver::Width
            } else {
                DimensionDriver::Height
            }
        }
    }
}

fn constrained_dimensions(
    driver: DimensionDriver,
    proposed: ResizeRect,
    layout: WindowLayout,
) -> (i32, i32) {
    match driver {
        DimensionDriver::Width => {
            let width = proposed.width().max(layout.width);
            let height = rounded_ratio(width, layout.height, layout.width).max(layout.height);
            let width = rounded_ratio(height, layout.width, layout.height).max(layout.width);
            (width, height)
        }
        DimensionDriver::Height => {
            let height = proposed.height().max(layout.height);
            let width = rounded_ratio(height, layout.width, layout.height).max(layout.width);
            let height = rounded_ratio(width, layout.height, layout.width).max(layout.height);
            (width, height)
        }
    }
}

fn constrain_sizing_rect(
    edge: SizingEdge,
    proposed: ResizeRect,
    current: ResizeRect,
    layout: WindowLayout,
) -> ResizeRect {
    let driver = dimension_driver(edge, proposed, current, layout);
    let (width, height) = constrained_dimensions(driver, proposed, layout);

    match edge {
        SizingEdge::Left => {
            let center_y = i64::from(current.top) + i64::from(current.height()) / 2;
            let top = center_y.saturating_sub(i64::from(height) / 2);
            let top = i32::try_from(top).unwrap_or(i32::MIN);
            ResizeRect::new(
                proposed.right.saturating_sub(width),
                top,
                proposed.right,
                top.saturating_add(height),
            )
        }
        SizingEdge::Right => {
            let center_y = i64::from(current.top) + i64::from(current.height()) / 2;
            let top = center_y.saturating_sub(i64::from(height) / 2);
            let top = i32::try_from(top).unwrap_or(i32::MIN);
            ResizeRect::new(
                proposed.left,
                top,
                proposed.left.saturating_add(width),
                top.saturating_add(height),
            )
        }
        SizingEdge::Top => {
            let center_x = i64::from(current.left) + i64::from(current.width()) / 2;
            let left = center_x.saturating_sub(i64::from(width) / 2);
            let left = i32::try_from(left).unwrap_or(i32::MIN);
            ResizeRect::new(
                left,
                proposed.bottom.saturating_sub(height),
                left.saturating_add(width),
                proposed.bottom,
            )
        }
        SizingEdge::Bottom => {
            let center_x = i64::from(current.left) + i64::from(current.width()) / 2;
            let left = center_x.saturating_sub(i64::from(width) / 2);
            let left = i32::try_from(left).unwrap_or(i32::MIN);
            ResizeRect::new(
                left,
                proposed.top,
                left.saturating_add(width),
                proposed.top.saturating_add(height),
            )
        }
        SizingEdge::TopLeft => ResizeRect::new(
            proposed.right.saturating_sub(width),
            proposed.bottom.saturating_sub(height),
            proposed.right,
            proposed.bottom,
        ),
        SizingEdge::TopRight => ResizeRect::new(
            proposed.left,
            proposed.bottom.saturating_sub(height),
            proposed.left.saturating_add(width),
            proposed.bottom,
        ),
        SizingEdge::BottomLeft => ResizeRect::new(
            proposed.right.saturating_sub(width),
            proposed.top,
            proposed.right,
            proposed.top.saturating_add(height),
        ),
        SizingEdge::BottomRight => ResizeRect::new(
            proposed.left,
            proposed.top,
            proposed.left.saturating_add(width),
            proposed.top.saturating_add(height),
        ),
    }
}

pub fn initialize(window: &WebviewWindow, layout: WindowLayout) -> Result<(), AppError> {
    platform::install(window, layout)?;
    apply_layout(window, layout)
}

pub fn apply_layout(window: &WebviewWindow, layout: WindowLayout) -> Result<(), AppError> {
    platform::update(window, layout)?;
    window.set_resizable(true)?;
    let size = LogicalSize::new(f64::from(layout.width), f64::from(layout.height));
    window.set_min_size(Some(size))?;
    window.set_size(size)?;
    Ok(())
}

#[cfg(windows)]
mod platform {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};

    use tauri::WebviewWindow;
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowRect, WMSZ_BOTTOM, WMSZ_BOTTOMLEFT, WMSZ_BOTTOMRIGHT, WMSZ_LEFT, WMSZ_RIGHT,
        WMSZ_TOP, WMSZ_TOPLEFT, WMSZ_TOPRIGHT, WM_NCDESTROY, WM_SIZING,
    };

    use super::{constrain_sizing_rect, ResizeRect, SizingEdge, WindowLayout};
    use crate::error::AppError;

    const ASPECT_RATIO_SUBCLASS_ID: usize = 0x5A48_454B;
    static WINDOW_LAYOUTS: OnceLock<Mutex<HashMap<usize, WindowLayout>>> = OnceLock::new();

    fn layouts() -> &'static Mutex<HashMap<usize, WindowLayout>> {
        WINDOW_LAYOUTS.get_or_init(|| Mutex::new(HashMap::new()))
    }

    fn native_handle(window: &WebviewWindow) -> Result<HWND, AppError> {
        let handle = window.hwnd()?;
        Ok(HWND(handle.0))
    }

    fn key(hwnd: HWND) -> usize {
        hwnd.0 as usize
    }

    fn sizing_edge(value: usize) -> Option<SizingEdge> {
        match value as u32 {
            WMSZ_LEFT => Some(SizingEdge::Left),
            WMSZ_RIGHT => Some(SizingEdge::Right),
            WMSZ_TOP => Some(SizingEdge::Top),
            WMSZ_BOTTOM => Some(SizingEdge::Bottom),
            WMSZ_TOPLEFT => Some(SizingEdge::TopLeft),
            WMSZ_TOPRIGHT => Some(SizingEdge::TopRight),
            WMSZ_BOTTOMLEFT => Some(SizingEdge::BottomLeft),
            WMSZ_BOTTOMRIGHT => Some(SizingEdge::BottomRight),
            _ => None,
        }
    }

    fn resize_rect(rect: RECT) -> ResizeRect {
        ResizeRect::new(rect.left, rect.top, rect.right, rect.bottom)
    }

    fn windows_rect(rect: ResizeRect) -> RECT {
        RECT {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        }
    }

    pub fn install(window: &WebviewWindow, layout: WindowLayout) -> Result<(), AppError> {
        let hwnd = native_handle(window)?;
        {
            let mut active = layouts().lock().map_err(|_| {
                AppError::Unknown("window aspect-ratio state is poisoned".to_string())
            })?;
            if active.contains_key(&key(hwnd)) {
                active.insert(key(hwnd), layout);
                return Ok(());
            }
            active.insert(key(hwnd), layout);
        }

        let installed = unsafe {
            SetWindowSubclass(
                hwnd,
                Some(aspect_ratio_subclass),
                ASPECT_RATIO_SUBCLASS_ID,
                0,
            )
        };
        if !installed.as_bool() {
            if let Ok(mut active) = layouts().lock() {
                active.remove(&key(hwnd));
            }
            return Err(AppError::Unknown(
                "failed to install the proportional resize handler".to_string(),
            ));
        }
        Ok(())
    }

    pub fn update(window: &WebviewWindow, layout: WindowLayout) -> Result<(), AppError> {
        let hwnd = native_handle(window)?;
        let mut active = layouts()
            .lock()
            .map_err(|_| AppError::Unknown("window aspect-ratio state is poisoned".to_string()))?;
        let current = active.get_mut(&key(hwnd)).ok_or_else(|| {
            AppError::Unknown("window aspect-ratio handler is not initialized".to_string())
        })?;
        *current = layout;
        Ok(())
    }

    unsafe extern "system" fn aspect_ratio_subclass(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _subclass_id: usize,
        _reference_data: usize,
    ) -> LRESULT {
        if message == WM_SIZING {
            if let Some(edge) = sizing_edge(wparam.0) {
                let layout = layouts()
                    .lock()
                    .ok()
                    .and_then(|active| active.get(&key(hwnd)).copied());
                let proposed_ptr = lparam.0 as *mut RECT;
                if let Some(layout) = layout {
                    if let Some(proposed) = unsafe { proposed_ptr.as_mut() } {
                        let mut current = RECT::default();
                        if unsafe { GetWindowRect(hwnd, &mut current) }.is_ok() {
                            let constrained = constrain_sizing_rect(
                                edge,
                                resize_rect(*proposed),
                                resize_rect(current),
                                layout,
                            );
                            *proposed = windows_rect(constrained);
                            return LRESULT(1);
                        }
                    }
                }
            }
        } else if message == WM_NCDESTROY {
            if let Ok(mut active) = layouts().lock() {
                active.remove(&key(hwnd));
            }
            unsafe {
                let _ = RemoveWindowSubclass(
                    hwnd,
                    Some(aspect_ratio_subclass),
                    ASPECT_RATIO_SUBCLASS_ID,
                );
            };
        }

        unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
    }
}

#[cfg(not(windows))]
mod platform {
    use tauri::WebviewWindow;

    use super::WindowLayout;
    use crate::error::AppError;

    pub fn install(_window: &WebviewWindow, _layout: WindowLayout) -> Result<(), AppError> {
        Ok(())
    }

    pub fn update(_window: &WebviewWindow, _layout: WindowLayout) -> Result<(), AppError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        constrain_sizing_rect, ResizeRect, SizingEdge, WindowLayout, MAIN_WINDOW_LAYOUT,
        UPDATE_WINDOW_LAYOUT,
    };

    fn assert_ratio_within_one_pixel(rect: ResizeRect, layout: WindowLayout) {
        let cross_error = ((i64::from(rect.width()) * i64::from(layout.height))
            - (i64::from(rect.height()) * i64::from(layout.width)))
        .abs();
        let tolerance = i64::from(layout.width.max(layout.height));
        assert!(
            cross_error <= tolerance,
            "rectangle {rect:?} does not preserve {layout:?}; cross error {cross_error}"
        );
    }

    #[test]
    fn right_edge_preserves_main_ratio_and_minimum() {
        let current = ResizeRect::new(0, 0, 892, 496);
        let proposed = ResizeRect::new(0, 0, 1_200, 496);
        let result =
            constrain_sizing_rect(SizingEdge::Right, proposed, current, MAIN_WINDOW_LAYOUT);

        assert_ratio_within_one_pixel(result, MAIN_WINDOW_LAYOUT);
        assert_eq!(result.left, 0);
        assert!(result.width() >= 892 && result.height() >= 496);
    }

    #[test]
    fn top_left_keeps_opposite_corner_anchored() {
        let current = ResizeRect::new(100, 100, 992, 596);
        let proposed = ResizeRect::new(0, 20, 992, 596);
        let result =
            constrain_sizing_rect(SizingEdge::TopLeft, proposed, current, MAIN_WINDOW_LAYOUT);

        assert_eq!((result.right, result.bottom), (992, 596));
        assert_ratio_within_one_pixel(result, MAIN_WINDOW_LAYOUT);
    }

    #[test]
    fn updater_ratio_uses_vertical_corner_motion_when_it_dominates() {
        let current = ResizeRect::new(0, 0, 788, 272);
        let proposed = ResizeRect::new(-10, -200, 788, 272);
        let result =
            constrain_sizing_rect(SizingEdge::TopLeft, proposed, current, UPDATE_WINDOW_LAYOUT);

        assert_ratio_within_one_pixel(result, UPDATE_WINDOW_LAYOUT);
        assert_eq!((result.right, result.bottom), (788, 272));
        assert!(result.height() > 272);
    }
}
