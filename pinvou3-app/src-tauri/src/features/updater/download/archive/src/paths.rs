use crate::ArchiveError;
use std::collections::BTreeMap;

struct State {
    original: String,
    file: bool,
    explicit: bool,
}
pub(crate) struct Registry {
    paths: BTreeMap<String, State>,
    charged: u64,
    limit: u64,
}
impl Registry {
    pub(crate) fn new(limit: u64) -> Self {
        Self {
            paths: BTreeMap::new(),
            charged: 0,
            limit,
        }
    }
    pub(crate) fn insert(&mut self, name: &str, directory: bool) -> Result<(), ArchiveError> {
        let path = if directory {
            name.strip_suffix('/').ok_or(ArchiveError::UnsafePath)?
        } else {
            name
        };
        if path.is_empty() || path.len() > 1024 {
            return Err(ArchiveError::UnsafePath);
        }
        let components: Vec<_> = path.split('/').collect();
        let mut end = 0;
        for (index, component) in components.iter().enumerate() {
            if component.is_empty()
                || *component == "."
                || *component == ".."
                || component.ends_with('.')
                || !component
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            {
                return Err(ArchiveError::UnsafePath);
            }
            let base = component.split('.').next().unwrap().to_ascii_uppercase();
            if ["CON", "PRN", "AUX", "NUL"].contains(&base.as_str())
                || (base.len() == 4
                    && (base.starts_with("COM") || base.starts_with("LPT"))
                    && (b'1'..=b'9').contains(&base.as_bytes()[3]))
            {
                return Err(ArchiveError::UnsafePath);
            }
            end += component.len() + usize::from(index != 0);
            let prefix = &path[..end];
            let key = prefix.to_ascii_lowercase();
            let last = index + 1 == components.len();
            if let Some(previous) = self.paths.get_mut(&key) {
                if previous.original != prefix
                    || previous.file
                    || (last && (previous.explicit || !directory))
                {
                    return Err(ArchiveError::UnsafePath);
                }
                if last {
                    previous.explicit = true;
                    previous.file = !directory;
                }
            } else {
                // Charge string storage and a conservative map/node allowance
                // before allocation; deep paths cannot expand registry memory
                // independently of the explicit path-state capacity.
                let charge = (prefix.len() as u64)
                    .checked_mul(2)
                    .and_then(|n| n.checked_add(256))
                    .ok_or(ArchiveError::ResourceLimit)?;
                self.charged = self
                    .charged
                    .checked_add(charge)
                    .filter(|n| *n <= self.limit)
                    .ok_or(ArchiveError::ResourceLimit)?;
                self.paths.insert(
                    key,
                    State {
                        original: prefix.to_string(),
                        file: last && !directory,
                        explicit: last,
                    },
                );
            }
        }
        Ok(())
    }
}
