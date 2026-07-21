//! Reusable numeric spinner backing the retail delivery/trade `All ◄ N/max ►`
//! quantity and gil pickers. Pure logic (no Bevy) so it is unit-testable; the
//! delivery screen renders [`Spinner::label`] and drives it from key input.

/// A numeric picker over `[min, max]`. `digits: Some` ⇒ direct decimal typing
/// (gil); `None` ⇒ arrow-only (item quantity). `step` is the Up/Down delta,
/// `jump` the Left/Right delta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spinner {
    pub value: u32,
    pub min: u32,
    pub max: u32,
    pub step: u32,
    pub jump: u32,
    pub digits: Option<String>,
}

impl Spinner {
    /// Item-quantity spinner over `1..=stack_max`. Defaults to 1 (retail shows
    /// `All ◄ 1/26 ►`); "All" jumps to the full stack. `jump` = full stack for a
    /// one-press Left/Right to either bound.
    pub fn item(stack_max: u32) -> Self {
        let max = stack_max.max(1);
        Self {
            value: 1,
            min: 1,
            max,
            step: 1,
            jump: max,
            digits: None,
        }
    }

    /// Gil spinner over `0..=current`. Digit typing; Up/Down ±1, Left/Right
    /// ±1000. Defaults to 0.
    pub fn gil(current: u32) -> Self {
        Self {
            value: 0,
            min: 0,
            max: current,
            step: 1,
            jump: 1000,
            digits: Some(String::new()),
        }
    }

    fn assign(&mut self, v: u32) {
        self.value = v.clamp(self.min, self.max);
        if let Some(d) = self.digits.as_mut() {
            *d = self.value.to_string();
        }
    }

    pub fn up(&mut self) {
        self.assign(self.value.saturating_add(self.step));
    }

    pub fn down(&mut self) {
        self.assign(self.value.saturating_sub(self.step));
    }

    pub fn jump_up(&mut self) {
        self.assign(self.value.saturating_add(self.jump));
    }

    pub fn jump_down(&mut self) {
        self.assign(self.value.saturating_sub(self.jump));
    }

    /// "All" affordance: select the full amount.
    pub fn set_all(&mut self) {
        self.assign(self.max);
    }

    /// Append a decimal digit (gil typing); non-digits ignored, result clamped
    /// to `max`.
    pub fn push_digit(&mut self, c: char) {
        let Some(digits) = self.digits.as_mut() else {
            return;
        };
        let Some(n) = c.to_digit(10) else {
            return;
        };
        // Ignore a leading zero so "0" then "5" reads as 5, not 05.
        if !(digits == "0" && n == 0) {
            if digits == "0" {
                digits.clear();
            }
            digits.push(char::from_digit(n, 10).expect("0..=9"));
        }
        let parsed = parse_u32(digits).min(self.max);
        self.assign(parsed);
    }

    /// Delete the last typed digit (gil typing).
    pub fn backspace(&mut self) {
        let Some(digits) = self.digits.as_mut() else {
            return;
        };
        digits.pop();
        let parsed = parse_u32(digits);
        // Re-derive value without rewriting `digits` (keep the user's in-progress
        // text, e.g. an empty field shows nothing rather than "0").
        self.value = parsed.clamp(self.min, self.max);
    }

    pub fn is_all(&self) -> bool {
        self.value == self.max
    }

    /// The confirmed amount to send.
    pub fn confirm(&self) -> u32 {
        self.value.clamp(self.min, self.max)
    }

    /// `All ◄ N/max ►` — "All" is dimmed unless the full amount is selected.
    /// (The screen decides styling; this is the plain text.)
    pub fn label(&self) -> String {
        format!("All ◄ {}/{} ►", self.value, self.max)
    }
}

fn parse_u32(s: &str) -> u32 {
    let mut v: u32 = 0;
    for c in s.chars() {
        if let Some(d) = c.to_digit(10) {
            v = v.saturating_mul(10).saturating_add(d);
        }
    }
    v
}

/// What a confirmed spinner value applies to. Both route to a delivery `Set`:
/// gil is inventory slot 0 (LSB stores gil as item 65535 at LOC_INVENTORY[0]),
/// items use their inventory index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpinnerTarget {
    ItemQty {
        inv_slot: u8,
        item_no: u16,
        out_slot: u8,
    },
    Gil {
        out_slot: u8,
    },
}

impl SpinnerTarget {
    /// LOC_INVENTORY index for the `Set` (gil = slot 0).
    pub fn inventory_slot(&self) -> u8 {
        match self {
            SpinnerTarget::ItemQty { inv_slot, .. } => *inv_slot,
            SpinnerTarget::Gil { .. } => 0,
        }
    }

    pub fn out_slot(&self) -> u8 {
        match self {
            SpinnerTarget::ItemQty { out_slot, .. } | SpinnerTarget::Gil { out_slot } => *out_slot,
        }
    }
}

/// An active spinner plus what it will stage on confirm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpinnerBinding {
    pub spinner: Spinner,
    pub target: SpinnerTarget,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_spinner_defaults_to_one_and_clamps() {
        let mut s = Spinner::item(26);
        assert_eq!(s.value, 1);
        assert_eq!(s.max, 26);
        s.down();
        assert_eq!(s.value, 1, "clamps at min");
        s.up();
        assert_eq!(s.value, 2);
        s.set_all();
        assert_eq!(s.value, 26);
        assert!(s.is_all());
        s.up();
        assert_eq!(s.value, 26, "clamps at max");
        assert_eq!(s.confirm(), 26);
    }

    #[test]
    fn item_spinner_stack_of_one() {
        let s = Spinner::item(1);
        assert_eq!(s.value, 1);
        assert_eq!(s.max, 1);
        assert!(s.is_all());
    }

    #[test]
    fn item_jump_hits_bounds_in_one_press() {
        let mut s = Spinner::item(99);
        s.jump_up();
        assert_eq!(s.value, 99);
        s.jump_down();
        assert_eq!(s.value, 1);
    }

    #[test]
    fn gil_spinner_types_and_clamps_to_current() {
        let mut s = Spinner::gil(15000);
        assert_eq!(s.value, 0);
        s.push_digit('1');
        s.push_digit('2');
        s.push_digit('0');
        s.push_digit('0');
        assert_eq!(s.value, 1200);
        // Overshoot clamps to available gil.
        s.push_digit('0');
        s.push_digit('0');
        assert_eq!(s.value, 15000);
        assert!(s.is_all());
        assert_eq!(s.confirm(), 15000);
    }

    #[test]
    fn gil_leading_zero_then_digit() {
        let mut s = Spinner::gil(999);
        s.push_digit('0');
        assert_eq!(s.value, 0);
        s.push_digit('5');
        assert_eq!(s.value, 5, "leading zero dropped");
    }

    #[test]
    fn gil_backspace() {
        let mut s = Spinner::gil(9999);
        for c in "1234".chars() {
            s.push_digit(c);
        }
        assert_eq!(s.value, 1234);
        s.backspace();
        assert_eq!(s.value, 123);
    }

    #[test]
    fn gil_jump_and_all() {
        let mut s = Spinner::gil(5000);
        s.jump_up();
        assert_eq!(s.value, 1000);
        s.jump_up();
        assert_eq!(s.value, 2000);
        s.set_all();
        assert_eq!(s.value, 5000);
    }

    #[test]
    fn target_maps_gil_to_inventory_slot_zero() {
        let g = SpinnerTarget::Gil { out_slot: 3 };
        assert_eq!(g.inventory_slot(), 0);
        assert_eq!(g.out_slot(), 3);
        let i = SpinnerTarget::ItemQty {
            inv_slot: 7,
            item_no: 4096,
            out_slot: 2,
        };
        assert_eq!(i.inventory_slot(), 7);
        assert_eq!(i.out_slot(), 2);
    }

    #[test]
    fn label_shows_all_marker() {
        let s = Spinner::item(26);
        assert_eq!(s.label(), "All ◄ 1/26 ►");
    }
}
