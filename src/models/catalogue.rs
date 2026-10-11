//! Embedded model catalogue and the RAM / speed matcher.
//!
//! `recommend` is pure: pass a [`Hardware`] value, don't probe the machine here.

use crate::models::hardware::{Accel, Hardware};
use anyhow::{bail, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Stt,
    Cleanup,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Model {
    pub id: String,
    pub kind: Kind,
    pub name: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
    pub licence: String,
    pub min_ram_mb: u64,
    pub languages: Vec<String>,
    pub realtime: Vec<Accel>,
}

impl Model {
    pub fn min_ram_bytes(&self) -> u64 {
        self.min_ram_mb.saturating_mul(1024 * 1024)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Catalogue {
    pub model: Vec<Model>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ranked<'a> {
    pub model: &'a Model,
    pub reason: String,
}

pub fn parse(text: &str) -> Result<Catalogue> {
    let cat: Catalogue = toml::from_str(text)?;
    for m in &cat.model {
        if m.sha256.len() != 64 || !m.sha256.chars().all(|c| c.is_ascii_hexdigit()) {
            bail!("{}: sha256 must be 64 hex chars", m.id);
        }
        if m.size == 0 {
            bail!("{}: size is 0", m.id);
        }
        if m.licence.is_empty() {
            bail!("{}: licence is empty", m.id);
        }
        if m.realtime.is_empty() {
            bail!("{}: realtime is empty", m.id);
        }
    }
    Ok(cat)
}

pub fn bundled() -> Catalogue {
    parse(include_str!("../../models.toml")).expect("models.toml")
}

/// About 60% of total RAM. A model whose working set is above this is not offered.
pub fn ram_budget(ram: u64) -> u64 {
    ram.saturating_mul(3) / 5
}

/// Largest real-time model for `kind` that fits in the RAM budget, best first.
/// A multilingual entry wins a tie against an English-only one.
pub fn recommend<'a>(hw: &Hardware, cat: &'a Catalogue, kind: Kind) -> Vec<Ranked<'a>> {
    let budget = ram_budget(hw.ram);
    let accel = Accel::for_gpu(hw.gpu);
    let mut fits: Vec<&Model> = cat
        .model
        .iter()
        .filter(|m| m.kind == kind && m.min_ram_bytes() <= budget && m.realtime.contains(&accel))
        .collect();
    fits.sort_by_key(|m| std::cmp::Reverse(rank_key(m)));
    fits.into_iter()
        .map(|model| Ranked {
            reason: reason(model, hw, budget, accel),
            model,
        })
        .collect()
}

fn rank_key(m: &Model) -> (u64, u8, u64) {
    let multi = u8::from(m.languages.iter().any(|l| l == "multi"));
    (m.min_ram_mb, multi, m.size)
}

fn reason(model: &Model, hw: &Hardware, budget: u64, accel: Accel) -> String {
    format!(
        "{} fits in {} GB of {} GB and is real-time on {}",
        model.id,
        gb(budget),
        gb(hw.ram),
        accel.as_str(),
    )
}

/// One decimal place, half up. `4 GiB` is `4.0`.
fn gb(bytes: u64) -> String {
    let gib = 1024u64 * 1024 * 1024;
    let tenths = bytes.saturating_mul(10).saturating_add(gib / 2) / gib;
    format!("{}.{}", tenths / 10, tenths % 10)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::hardware::GpuKind;

    #[test]
    fn bundled_entries_have_sha_size_and_licence() {
        let cat = bundled();
        assert!(cat.model.len() >= 10, "{}", cat.model.len());
        let stt = cat.model.iter().filter(|m| m.kind == Kind::Stt).count();
        let cleanup = cat.model.iter().filter(|m| m.kind == Kind::Cleanup).count();
        assert_eq!(stt, 8);
        assert_eq!(cleanup, 2);
        for m in &cat.model {
            assert_eq!(m.sha256.len(), 64, "{}", m.id);
            assert!(m.size > 0, "{}", m.id);
            assert!(!m.licence.is_empty(), "{}", m.id);
        }
    }

    #[test]
    fn bad_sha_is_an_error() {
        let err = parse(
            r#"
            [[model]]
            id = "x"
            kind = "stt"
            name = "x"
            url = "https://example.test/x"
            size = 1
            sha256 = "abcd"
            licence = "MIT"
            min_ram_mb = 1
            languages = ["en"]
            realtime = ["cpu"]
            "#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("sha256"), "{err}");
    }

    fn machine(gb: u64, gpu: GpuKind) -> Hardware {
        Hardware {
            ram: gb * 1024 * 1024 * 1024,
            free_disk: 0,
            cpu_cores: 8,
            gpu,
        }
    }

    #[test]
    fn recommend_picks_for_the_four_machines() {
        let cat = bundled();
        let cases = [
            (8, GpuKind::None, "ggml-base", "cpu"),
            (16, GpuKind::Metal, "ggml-medium", "metal"),
            (4, GpuKind::None, "ggml-base", "cpu"),
            (64, GpuKind::Cuda, "ggml-medium", "cuda"),
        ];
        for (gb, gpu, id, accel) in cases {
            let hw = machine(gb, gpu);
            let ranked = recommend(&hw, &cat, Kind::Stt);
            let top = ranked.first().unwrap_or_else(|| panic!("{gb} GB {gpu:?}"));
            assert_eq!(top.model.id, id, "{gb} GB {gpu:?}: {}", top.reason);
            assert!(
                top.reason.contains(accel) && top.reason.contains(id),
                "{}",
                top.reason
            );
            let budget = ram_budget(hw.ram);
            assert!(
                ranked.iter().all(|r| r.model.min_ram_bytes() <= budget),
                "{gb} GB offered a model over the budget"
            );
            let cleanup = recommend(&hw, &cat, Kind::Cleanup);
            assert_eq!(cleanup[0].model.id, "s1-mini-q4");
        }
    }

    #[test]
    fn a_tight_budget_drops_the_larger_cleanup_model() {
        let cat = bundled();
        // 2 GB * 60% is under s1-mini's 1500 MB and above SmolLM2's 800 MB.
        let hw = machine(2, GpuKind::None);
        let ranked = recommend(&hw, &cat, Kind::Cleanup);
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].model.id, "smollm2-360m-q4");
    }
}
