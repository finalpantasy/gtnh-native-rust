use std::collections::BTreeSet;
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Key {
    W,
    A,
    S,
    D,
    Space,
    Control,
    Left,
    Right,
    Up,
    Down,
    Plus,
    Minus,
    P,
    N,
    E,
    Home,
    Escape,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    ToggleRun,
    Step,
    Submit,
    Home,
    Save,
    Quit,
}
#[derive(Default)]
pub struct Input {
    held: BTreeSet<Key>,
    control_shortcut: bool,
}
impl Input {
    pub fn key(&mut self, key: Key, pressed: bool, repeat: bool, consumed: bool) -> Option<Action> {
        if !pressed {
            self.held.remove(&key);
            if key == Key::Control {
                self.control_shortcut = false;
            }
            return None;
        }
        if consumed {
            return None;
        }
        if self.held.contains(&Key::Control) && key == Key::S {
            self.held.remove(&Key::S);
            self.control_shortcut = true;
            return if repeat { None } else { Some(Action::Save) };
        }
        let action = match key {
            Key::P => Some(Action::ToggleRun),
            Key::N => Some(Action::Step),
            Key::E => Some(Action::Submit),
            Key::Home => Some(Action::Home),
            Key::Escape => Some(Action::Quit),
            _ => None,
        };
        if action.is_some() {
            return if repeat { None } else { action };
        }
        self.held.insert(key);
        None
    }
    pub fn held(&self, key: Key) -> bool {
        self.held.contains(&key) && !(key == Key::Control && self.control_shortcut)
    }
    pub fn focus_lost(&mut self) {
        self.held.clear();
        self.control_shortcut = false;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn one_shot_actions_ignore_auto_repeat_and_ui_consumption() {
        let mut input = Input::default();
        assert_eq!(
            input.key(Key::P, true, false, false),
            Some(Action::ToggleRun)
        );
        assert_eq!(input.key(Key::P, true, true, false), None);
        assert_eq!(input.key(Key::E, true, false, true), None);
        assert_eq!(input.key(Key::E, true, false, false), Some(Action::Submit));
        assert_eq!(input.key(Key::N, true, false, false), Some(Action::Step));
        assert_eq!(
            input.key(Key::Escape, true, false, false),
            Some(Action::Quit)
        );
    }
    #[test]
    fn focus_and_consumed_key_release_cannot_leave_camera_motion_stuck() {
        let mut input = Input::default();
        input.key(Key::W, true, false, false);
        assert!(input.held(Key::W));
        input.key(Key::W, false, false, true);
        assert!(!input.held(Key::W));
        input.key(Key::D, true, false, false);
        input.focus_lost();
        assert!(!input.held(Key::D));
    }
    #[test]
    fn control_s_saves_without_starting_camera_backwards() {
        let mut input = Input::default();
        input.key(Key::Control, true, false, false);
        assert_eq!(input.key(Key::S, true, false, false), Some(Action::Save));
        assert!(!input.held(Key::S));
    }
}
