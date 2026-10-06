pub const DICTIONARY_BYTES: usize = 16 * 1024;
pub const DEFAULT_DICTIONARY: &str = include_str!("../../../assets/input/pinyin.tsv");
pub const MAX_CANDIDATES: usize = 8;

pub enum Input {
    Pass,
    Consumed,
    Commit { text: [u8; 64], length: usize },
}

pub struct Composition {
    pub enabled: bool,
    owner: u32,
    buffer: [u8; 32],
    length: usize,
    selected: usize,
    dictionary: &'static str,
}

impl Composition {
    pub const fn new(dictionary: &'static str) -> Self {
        Self {
            enabled: false,
            owner: 0,
            buffer: [0; 32],
            length: 0,
            selected: 0,
            dictionary,
        }
    }

    pub fn focus(&mut self, window: u32) -> bool {
        if self.owner == window {
            return false;
        }
        self.owner = window;
        let changed = self.length != 0;
        self.clear();
        changed
    }

    pub fn text(&self) -> &str {
        core::str::from_utf8(&self.buffer[..self.length]).unwrap_or("")
    }

    pub fn candidates(&self) -> [&'static str; MAX_CANDIDATES] {
        let mut matches = [""; MAX_CANDIDATES];
        if self.length == 0 {
            return matches;
        }
        let mut count = 0;
        for line in self.dictionary.lines() {
            let Some((key, text)) = line.split_once('\t') else {
                continue;
            };
            if key == self.text()
                && !text.is_empty()
                && text.len() <= 64
                && text.chars().count() <= 8
            {
                matches[count] = text;
                count += 1;
                if count == matches.len() {
                    break;
                }
            }
        }
        matches
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn key(
        &mut self,
        code: u16,
        down: bool,
        control: bool,
        shift: bool,
        alt: bool,
        letter: Option<u8>,
    ) -> Input {
        if code == 57 && down && control && !alt {
            self.enabled = !self.enabled;
            self.clear();
            return Input::Consumed;
        }
        if !self.enabled || control || alt || matches!(code, 29 | 42 | 54 | 97) {
            return Input::Pass;
        }
        if !down {
            return if self.length != 0 {
                Input::Consumed
            } else {
                Input::Pass
            };
        }
        if code == 1 {
            self.clear();
            return Input::Consumed;
        }
        if code == 14 && self.length != 0 {
            self.length -= 1;
            self.selected = 0;
            return Input::Consumed;
        }
        if self.length != 0 {
            let candidates = self.candidates();
            let count = candidates
                .iter()
                .take_while(|text| !text.is_empty())
                .count();
            if matches!(code, 105 | 106) && count != 0 {
                self.selected = if code == 105 {
                    (self.selected + count - 1) % count
                } else {
                    (self.selected + 1) % count
                };
                return Input::Consumed;
            }
            let selection = match code {
                28 | 57 => Some(self.selected),
                2..=9 if !shift => Some(code as usize - 2),
                _ => None,
            };
            if let Some(selection) = selection {
                let mut text = [0; 64];
                let source = candidates
                    .get(selection)
                    .copied()
                    .filter(|text| !text.is_empty())
                    .unwrap_or_else(|| self.text());
                let length = source.len();
                text[..length].copy_from_slice(source.as_bytes());
                self.clear();
                return Input::Commit { text, length };
            }
        }
        if let Some(byte) = letter.filter(u8::is_ascii_lowercase) {
            if self.length < self.buffer.len() {
                self.buffer[self.length] = byte;
                self.length += 1;
                self.selected = 0;
            }
            return Input::Consumed;
        }
        if self.length != 0 {
            Input::Consumed
        } else {
            Input::Pass
        }
    }

    fn clear(&mut self) {
        self.length = 0;
        self.selected = 0;
    }
}
