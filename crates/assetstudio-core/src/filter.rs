use anyhow::{Context, Result, bail};
use regex::{Regex, RegexBuilder};

use crate::AssetRecord;

/// OR within each filter group, AND between class/name/container groups.
#[derive(Debug, Default)]
pub struct AssetFilter {
    classes: Vec<String>,
    names: Vec<Regex>,
    containers: Vec<Regex>,
}

impl AssetFilter {
    pub fn new(classes: &[String], names: &[String], containers: &[String]) -> Result<Self> {
        if classes.iter().any(|value| value.trim().is_empty()) {
            bail!("class filters must not be empty");
        }
        Ok(Self {
            classes: classes
                .iter()
                .map(|name| crate::class_id(name).map_or_else(|| name.clone(), |id| id.to_string()))
                .collect(),
            names: compile(names)?,
            containers: compile(containers)?,
        })
    }

    pub fn matches(&self, record: &AssetRecord) -> bool {
        (self.classes.is_empty()
            || self.classes.iter().any(|value| {
                value.eq_ignore_ascii_case(&record.class_name)
                    || value.parse::<i32>().ok() == Some(record.class_id)
            }))
            && (self.names.is_empty()
                || self
                    .names
                    .iter()
                    .any(|filter| filter.is_match(&record.name)))
            && (self.containers.is_empty()
                || self
                    .containers
                    .iter()
                    .any(|filter| filter.is_match(&record.container)))
    }
}

fn compile(patterns: &[String]) -> Result<Vec<Regex>> {
    patterns
        .iter()
        .map(|pattern| {
            if pattern.trim().is_empty() {
                bail!("regular expression filters must not be empty");
            }
            RegexBuilder::new(pattern)
                .case_insensitive(true)
                .size_limit(4 * 1024 * 1024)
                .build()
                .with_context(|| format!("invalid regular expression {pattern:?}"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AssetKey;
    fn record() -> AssetRecord {
        AssetRecord {
            key: AssetKey {
                file_index: 0,
                path_id: 1,
            },
            class_id: 28,
            class_name: "Texture2D".into(),
            name: "Hello".into(),
            container: "ui/HELLO.png".into(),
            source: "CAB-123".into(),
            size: 16,
        }
    }
    #[test]
    fn filters_and_groups_and_ignore_case() {
        let filter = AssetFilter::new(
            &["Texture2D".into(), "43".into()],
            &["^hel".into()],
            &["ui/hello".into()],
        )
        .unwrap();
        assert!(filter.matches(&record()));
        let filter = AssetFilter::new(&["43".into()], &[], &[]).unwrap();
        assert!(!filter.matches(&record()));
    }
    #[test]
    fn invalid_and_empty_patterns_are_errors() {
        assert!(AssetFilter::new(&[], &["[".into()], &[]).is_err());
        assert!(AssetFilter::new(&[], &[" ".into()], &[]).is_err());
        assert!(AssetFilter::new(&[], &[], &["".into()]).is_err());
    }

    #[test]
    fn original_class_alias_is_accepted() {
        let mut asset = record();
        asset.class_id = 126;
        asset.class_name = crate::class_name(126).to_owned();
        assert!(
            AssetFilter::new(&["NavMeshProjectSettings".into()], &[], &[])
                .unwrap()
                .matches(&asset)
        );
        assert!(
            AssetFilter::new(&["NavMeshAreas".into()], &[], &[])
                .unwrap()
                .matches(&asset)
        );
    }
}
