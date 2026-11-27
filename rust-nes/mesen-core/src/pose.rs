//! Opt-in original sprite bytes retained from real PPU fetches.

#[derive(Clone, Default)]
pub struct PoseCell {
    pub x: u8,
    pub y: u8,
    pub flags: u8,
    pub index: u16,
    pub palette: [u8; 3],
    pub pattern: [u8; 16],
    rows: u8,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publishes_complete_original_rows_only() {
        let mut observer = PoseObserver::default();
        observer.arm(true);
        for row in 0..8 {
            observer.row(30, 50, 64, 0x1000 + row, [1, 2, 3], row as u8, 255);
        }
        observer.row(60, 50, 0, 0x1020, [1, 2, 3], 7, 8);
        observer.finish();
        let cells: Vec<_> = observer.cells().collect();
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].index, 256);
        assert_eq!(&cells[0].pattern[..8], &[0, 1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(&cells[0].pattern[8..], &[255; 8]);
        observer.arm(false);
        assert_eq!(observer.cells().count(), 0);
    }

    #[test]
    fn inactive_observer_retains_nothing_and_active_storage_is_bounded() {
        let mut observer = PoseObserver::default();
        observer.row(10, 10, 0, 0, [1, 2, 3], 255, 255);
        assert!(observer.pending.is_empty());
        observer.arm(true);
        for x in 0..255 {
            observer.row(x, 10, 0, 0, [1, 2, 3], 255, 255);
        }
        assert_eq!(observer.pending.len(), 128);
    }
}

pub struct PoseObserver {
    active: bool,
    pending: Vec<PoseCell>,
    complete: Vec<PoseCell>,
    camera_pending: Option<(u16, u16)>,
    camera_complete: Option<(u16, u16)>,
}

impl Default for PoseObserver {
    fn default() -> Self {
        Self { active: false, pending: Vec::new(), complete: Vec::new(),
            camera_pending: None, camera_complete: None }
    }
}

impl PoseObserver {
    pub fn active(&self) -> bool { self.active }
    pub fn arm(&mut self, active: bool) {
        if self.active != active {
            self.active = active;
            self.clear();
        }
    }

    pub fn clear(&mut self) {
        self.pending.clear();
        self.complete.clear();
        self.camera_pending = None;
        self.camera_complete = None;
    }

    pub fn begin(&mut self) { self.pending.clear(); self.camera_pending = None; }

    pub fn camera_sample(&mut self, x: u16, y: u16) { self.camera_pending = Some((x, y)); }
    pub fn camera(&self) -> Option<(u16, u16)> { self.camera_complete }

    pub fn finish(&mut self) {
        if self.active {
            std::mem::swap(&mut self.pending, &mut self.complete);
            self.pending.clear();
            self.camera_complete = self.camera_pending;
        }
    }

    pub fn cells(&self) -> impl Iterator<Item = &PoseCell> {
        self.complete.iter().filter(|cell| cell.rows == 255)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn row(&mut self, x: u8, y: u8, flags: u8, addr: u16,
               palette: [u8; 3], low: u8, high: u8) {
        if !self.active || y >= 232 { return; }
        let index = addr / 16;
        let row = (addr & 7) as usize;
        let position = self.pending.iter().position(|cell|
            cell.x == x && cell.y == y && cell.flags == flags
                && cell.index == index && cell.palette == palette);
        let slot = match position {
            Some(slot) => slot,
            None if self.pending.len() < 128 => {
                self.pending.push(PoseCell { x, y, flags, index, palette, ..PoseCell::default() });
                self.pending.len() - 1
            }
            None => return,
        };
        let cell = &mut self.pending[slot];
        cell.pattern[row] = low;
        cell.pattern[row + 8] = high;
        cell.rows |= 1 << row;
    }
}
