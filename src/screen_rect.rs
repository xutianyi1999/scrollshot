use windows::Win32::Foundation::POINT;

#[derive(Clone, Copy, Debug)]
pub struct ScreenRect {
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
}

impl ScreenRect {
    pub fn from_points(a: POINT, b: POINT) -> Option<Self> {
        let left = a.x.min(b.x);
        let top = a.y.min(b.y);
        let right = a.x.max(b.x);
        let bottom = a.y.max(b.y);
        let width = right - left;
        let height = bottom - top;

        (width > 0 && height > 0).then_some(Self {
            left,
            top,
            width,
            height,
        })
    }

    pub fn right(&self) -> i32 {
        self.left + self.width
    }

    pub fn bottom(&self) -> i32 {
        self.top + self.height
    }
}
