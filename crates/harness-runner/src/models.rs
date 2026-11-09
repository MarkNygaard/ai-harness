//! One table of the models the harness knows, and what they cost.
//!
//! Adding a model used to mean editing three places in two languages: the
//! editor's catalog (twice for a Codex model — once bare, once `openai-codex/`
//! prefixed), a price arm in Rust, and the same price arm again in TypeScript.
//! Nothing made them fail together when they drifted.
//!
//! **A row per `(provider, id)`, naming a family.** Not one model with a
//! transform per agent: the ids are not related by one. Claude Code's `sonnet`
//! is Cursor's `claude-sonnet-5`, and Codex's `gpt-5.6-sol` is omp's
//! `openai-codex/gpt-5.6-sol` — a prefix in that one case and nothing like it
//! in the other. Anything derived here would be true of one pair and wrong
//! about the rest, so nothing is derived.
//!
//! **Prefer an alias to a version.** Claude Code's `sonnet` always resolves to
//! the newest Sonnet, which is why that entry has survived four model
//! generations untouched while Cursor's pinned `sonnet-4` rotted into an id
//! their API no longer has. List the alias where an agent offers one; pin a
//! version only where a workflow needs that exact model.
//!
//! **This is a catalog, not a gate.** Any model string is accepted — a workflow
//! can pin an id that is not here, and the editor keeps a node's current model
//! whether or not it is listed. So pricing falls back to matching on the id
//! when it is not in the table, and the table is what makes the *listed* ones
//! exact.

/// Per-MTok USD rates.
///
/// A notional cost basis — comparable across subscription and API-billed runs,
/// and deliberately not an invoice.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct Rates {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

impl Rates {
    /// Notional USD cost of a token breakdown at these rates.
    pub fn cost_usd(&self, input: u64, output: u64, cache_read: u64, cache_write: u64) -> f64 {
        (input as f64 * self.input
            + output as f64 * self.output
            + cache_read as f64 * self.cache_read
            + cache_write as f64 * self.cache_write)
            / 1_000_000.0
    }
}

/// What a model costs and which subscription pays for it, independent of the
/// agent that reaches it — `sonnet` on Claude Code and `claude-sonnet-5` on
/// Cursor are the same model at the same price.
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct Family {
    pub id: &'static str,
    /// Coarse bucket a subscription is measured in: `claude`, `gpt`, `kimi`,
    /// `composer`, `other`.
    pub lane: &'static str,
    pub rates: Rates,
}

const fn rates(input: f64, output: f64, cache_read: f64, cache_write: f64) -> Rates {
    Rates {
        input,
        output,
        cache_read,
        cache_write,
    }
}

/// Every pricing family. Adding a model that fits one of these is a row in
/// [`MODELS`] and nothing else.
pub const FAMILIES: &[Family] = &[
    Family {
        id: "opus",
        lane: "claude",
        rates: rates(5.0, 25.0, 0.5, 6.25),
    },
    Family {
        id: "sonnet",
        lane: "claude",
        rates: rates(3.0, 15.0, 0.3, 3.75),
    },
    Family {
        id: "haiku",
        lane: "claude",
        rates: rates(1.0, 5.0, 0.1, 1.25),
    },
    Family {
        id: "fable",
        lane: "claude",
        rates: rates(10.0, 50.0, 1.0, 12.5),
    },
    // GPT-6 Astra, standard tier at short context (<= 272K input). A request
    // above that threshold prices at long-context rates; not modelled, since
    // the id carries no context tier.
    Family {
        id: "gpt-6",
        lane: "gpt",
        rates: rates(10.0, 50.0, 1.0, 12.5),
    },
    // The 5.6 tiers are 25x apart end to end, so each gets its own family —
    // pricing Luna at Sol's rate would make an A/B between them meaningless.
    Family {
        id: "gpt-5.6-sol",
        lane: "gpt",
        rates: rates(5.0, 30.0, 0.5, 5.0),
    },
    Family {
        id: "gpt-5.6-terra",
        lane: "gpt",
        rates: rates(2.0, 12.0, 0.2, 2.5),
    },
    Family {
        id: "gpt-5.6-luna",
        lane: "gpt",
        rates: rates(0.2, 1.2, 0.02, 0.25),
    },
    // The rest of the gpt-5.x line, at Sol's rate.
    Family {
        id: "gpt-5",
        lane: "gpt",
        rates: rates(5.0, 30.0, 0.5, 5.0),
    },
    Family {
        id: "kimi",
        lane: "kimi",
        rates: rates(0.95, 4.0, 0.16, 0.95),
    },
    // Cursor Composer 2.5 standard tier. cache_read dominates a coding run, so
    // that is the figure reconciling notional cost with Cursor's dashboard. No
    // write-cache rate is published; input-rate is a safe upper bound.
    Family {
        id: "composer",
        lane: "composer",
        rates: rates(0.5, 2.5, 0.2, 0.5),
    },
    Family {
        id: "grok",
        lane: "composer",
        rates: rates(0.5, 2.5, 0.2, 0.5),
    },
    Family {
        id: "gemini",
        lane: "other",
        rates: rates(2.0, 12.0, 0.2, 2.5),
    },
];

/// One model as one agent names it.
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct Model {
    /// Provider id: `claude`, `codex`, `pi`, `cursor`, `anthropic-api`.
    pub provider: &'static str,
    /// The exact string handed to that agent's CLI.
    pub id: &'static str,
    /// Which [`Family`] prices it.
    pub family: &'static str,
    /// Covered by the provider's subscription rather than metered on top.
    ///
    /// Cursor is the case that makes this worth recording: its own models are
    /// included, while Claude and GPT through Cursor draw from a separate pool
    /// *charged at the model's API price*. Both work without a separate API
    /// key, which is exactly why the difference is invisible until an invoice.
    pub included: bool,
    /// The oldest CLI version that knows this model, when an older one would
    /// reject it. The editor lists such a model as unavailable, with this
    /// version, while the installed CLI is older — so a new model can be
    /// announced before the image catches up, and nobody picks it into a node
    /// that cannot run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_cli: Option<&'static str>,
}

/// Every model the harness offers, per agent.
///
/// Order within a provider is the order the editor shows them, so the one you
/// most likely want comes first.
pub const MODELS: &[Model] = &[
    // ── Claude Code ─────────────────────────────────────────────────────────
    // Aliases: each always resolves to the newest of its family.
    Model {
        provider: "claude",
        id: "sonnet",
        family: "sonnet",
        included: true,
        min_cli: None,
    },
    Model {
        provider: "claude",
        id: "opus",
        family: "opus",
        included: true,
        min_cli: None,
    },
    Model {
        provider: "claude",
        id: "haiku",
        family: "haiku",
        included: true,
        min_cli: None,
    },
    Model {
        provider: "claude",
        id: "fable",
        family: "fable",
        included: true,
        min_cli: None,
    },
    // ── Codex CLI on a ChatGPT account ──────────────────────────────────────
    // The general models, not the `-codex` variants, which need API-key auth.
    // Versioned because Codex publishes no alias for these tiers.
    Model {
        provider: "codex",
        id: "gpt-6-astra",
        family: "gpt-6",
        included: true,
        min_cli: None,
    },
    Model {
        provider: "codex",
        id: "gpt-5.6-sol",
        family: "gpt-5.6-sol",
        included: true,
        min_cli: None,
    },
    Model {
        provider: "codex",
        id: "gpt-5.6-terra",
        family: "gpt-5.6-terra",
        included: true,
        min_cli: None,
    },
    Model {
        provider: "codex",
        id: "gpt-5.6-luna",
        family: "gpt-5.6-luna",
        included: true,
        min_cli: None,
    },
    Model {
        provider: "codex",
        id: "gpt-5.5",
        family: "gpt-5",
        included: true,
        min_cli: None,
    },
    Model {
        provider: "codex",
        id: "gpt-5.4",
        family: "gpt-5",
        included: true,
        min_cli: None,
    },
    Model {
        provider: "codex",
        id: "gpt-5.4-mini",
        family: "gpt-5",
        included: true,
        min_cli: None,
    },
    // ── omp (`pi`) ──────────────────────────────────────────────────────────
    // omp rejects a bare OpenAI id — `resolve_model` mis-prefixes it to
    // `kimi-code/` — so these carry the backend prefix.
    Model {
        provider: "pi",
        id: "openai-codex/gpt-6-astra",
        family: "gpt-6",
        included: true,
        // omp 18.1.12 added gpt-6-astra to its openai-codex catalog (see the
        // OMP_VERSION pin in the Dockerfile).
        min_cli: Some("18.1.12"),
    },
    Model {
        provider: "pi",
        id: "openai-codex/gpt-5.6-sol",
        family: "gpt-5.6-sol",
        included: true,
        min_cli: None,
    },
    Model {
        provider: "pi",
        id: "openai-codex/gpt-5.6-terra",
        family: "gpt-5.6-terra",
        included: true,
        min_cli: None,
    },
    Model {
        provider: "pi",
        id: "openai-codex/gpt-5.6-luna",
        family: "gpt-5.6-luna",
        included: true,
        min_cli: None,
    },
    Model {
        provider: "pi",
        id: "openai-codex/gpt-5.5",
        family: "gpt-5",
        included: true,
        min_cli: None,
    },
    Model {
        provider: "pi",
        id: "openai-codex/gpt-5.4-nano",
        family: "gpt-5",
        included: true,
        min_cli: None,
    },
    Model {
        provider: "pi",
        id: "openai-codex/gpt-5.2-codex",
        family: "gpt-5",
        included: true,
        min_cli: None,
    },
    Model {
        provider: "pi",
        id: "openai-codex/gpt-5.1-codex-max",
        family: "gpt-5",
        included: true,
        min_cli: None,
    },
    Model {
        provider: "pi",
        id: "openai-codex/gpt-5.1-codex",
        family: "gpt-5",
        included: true,
        min_cli: None,
    },
    Model {
        provider: "pi",
        id: "kimi-code/kimi-for-coding",
        family: "kimi",
        included: true,
        min_cli: None,
    },
    Model {
        provider: "pi",
        id: "kimi-code/kimi-k2",
        family: "kimi",
        included: true,
        min_cli: None,
    },
    Model {
        provider: "pi",
        id: "kimi-code/kimi-k2-turbo-preview",
        family: "kimi",
        included: true,
        min_cli: None,
    },
    Model {
        provider: "pi",
        id: "kimi-code/kimi-k2.5",
        family: "kimi",
        included: true,
        min_cli: None,
    },
    // ── Cursor ──────────────────────────────────────────────────────────────
    // Cursor's own models are included in the subscription; everything below
    // them draws from the "Other Models" pool at the model's API price.
    Model {
        provider: "cursor",
        id: "composer-2.5",
        family: "composer",
        included: true,
        min_cli: None,
    },
    Model {
        provider: "cursor",
        id: "composer-2.5-fast",
        family: "composer",
        included: true,
        min_cli: None,
    },
    Model {
        provider: "cursor",
        id: "grok-4.6",
        family: "grok",
        included: true,
        min_cli: None,
    },
    Model {
        provider: "cursor",
        id: "grok-4.6-fast",
        family: "grok",
        included: true,
        min_cli: None,
    },
    Model {
        provider: "cursor",
        id: "claude-sonnet-5",
        family: "sonnet",
        included: false,
        min_cli: None,
    },
    Model {
        provider: "cursor",
        id: "claude-opus-5",
        family: "opus",
        included: false,
        min_cli: None,
    },
    Model {
        provider: "cursor",
        id: "claude-fable-5.1",
        family: "fable",
        included: false,
        min_cli: None,
    },
    Model {
        provider: "cursor",
        id: "claude-4.5-haiku",
        family: "haiku",
        included: false,
        min_cli: None,
    },
    Model {
        provider: "cursor",
        id: "gpt-5.6-sol",
        family: "gpt-5.6-sol",
        included: false,
        min_cli: None,
    },
    Model {
        provider: "cursor",
        id: "gpt-5.6-terra",
        family: "gpt-5.6-terra",
        included: false,
        min_cli: None,
    },
    Model {
        provider: "cursor",
        id: "gpt-5.6-luna",
        family: "gpt-5.6-luna",
        included: false,
        min_cli: None,
    },
    Model {
        provider: "cursor",
        id: "gpt-5.5",
        family: "gpt-5",
        included: false,
        min_cli: None,
    },
    Model {
        provider: "cursor",
        id: "gemini-3-pro",
        family: "gemini",
        included: false,
        min_cli: None,
    },
    // ── Direct Anthropic API (API key, not a subscription) ──────────────────
    Model {
        provider: "anthropic-api",
        id: "sonnet",
        family: "sonnet",
        included: false,
        min_cli: None,
    },
    Model {
        provider: "anthropic-api",
        id: "opus",
        family: "opus",
        included: false,
        min_cli: None,
    },
];

/// The ids one provider offers, in listing order, overlay included.
pub fn models_for(provider: &str) -> Vec<&'static str> {
    model_rows(provider).into_iter().map(|m| m.id).collect()
}

/// The rows one provider offers, in listing order, overlay included.
pub fn model_rows(provider: &str) -> Vec<Model> {
    effective()
        .models
        .iter()
        .filter(|m| m.provider == provider)
        .copied()
        .collect()
}

/// How to price a model nobody listed, by what its id contains.
///
/// **Order matters and is the whole subtlety.** `gpt-6` and the 5.6 tiers come
/// before the general `gpt-5` arm, which `openai-codex/gpt-6-astra` and
/// `gpt-5.6-luna` would otherwise also match — and pricing Luna at Sol's rate
/// would make an A/B between them meaningless when they are 25x apart.
///
/// Data rather than a chain of `else if` because the web prices runs too, and a
/// second hand-written chain in TypeScript is what this table exists to stop.
pub const FALLBACKS: &[(&str, &str)] = &[
    ("opus", "opus"),
    ("haiku", "haiku"),
    ("fable", "fable"),
    ("sonnet", "sonnet"),
    ("gpt-6", "gpt-6"),
    ("5.6-terra", "gpt-5.6-terra"),
    ("5.6-luna", "gpt-5.6-luna"),
    ("gpt-5", "gpt-5"),
    ("codex", "gpt-5"),
    ("openai", "gpt-5"),
    ("kimi", "kimi"),
    ("moonshot", "kimi"),
    ("composer", "composer"),
    ("grok", "grok"),
    ("gemini", "gemini"),
];

/// What an id matching nothing prices at. Sonnet tier, the long-standing
/// default — a number somebody can argue with, rather than zero, which would
/// read as a free run.
pub const DEFAULT_FAMILY: &str = "sonnet";

/// The family that prices a model: by exact id, then by the id's shape.
///
/// The fallback is not a leftover. Any model string is accepted — a workflow
/// can pin an id nobody listed, and the editor keeps a node's current model
/// whether or not it is in the table — so this always answers.
pub fn family_for(model: &str) -> Family {
    let table = effective();
    if let Some(m) = table
        .models
        .iter()
        .find(|m| m.id.eq_ignore_ascii_case(model))
    {
        return table.family(m.family);
    }
    let lower = model.to_ascii_lowercase();
    let id = FALLBACKS
        .iter()
        .find(|(needle, _)| lower.contains(needle))
        .map(|(_, family)| *family)
        .unwrap_or(DEFAULT_FAMILY);
    table.family(id)
}

/// The whole table, for generating the copy the web prices from.
///
/// Serialised rather than served: the run overview prices per node during
/// render, so fetching rates would mean either restructuring that or holding a
/// second table as a pre-load fallback — and a second table is the thing being
/// removed.
#[derive(Debug, serde::Serialize)]
pub struct Catalog {
    pub families: &'static [Family],
    pub models: &'static [Model],
    pub fallbacks: &'static [(&'static str, &'static str)],
    pub default_family: &'static str,
}

/// This table as JSON, pretty-printed for a reviewable diff.
pub fn catalog_json() -> String {
    serde_json::to_string_pretty(&Catalog {
        families: FAMILIES,
        models: MODELS,
        fallbacks: FALLBACKS,
        default_family: DEFAULT_FAMILY,
    })
    .expect("model catalog serialises")
}

// ── The overlay ─────────────────────────────────────────────────────────────
//
// A JSON file that adds models and families to the table above, or changes
// them, without a new image. The bundled table is still what ships and what
// the web's generated copy is made from; the overlay is for the gap between a
// model coming out and a release that lists it.

/// Where the overlay is read from: this variable's path, else
/// `$HARNESS_DATA_DIR/model-catalog.json` when that file exists.
pub const CATALOG_ENV: &str = "HARNESS_MODEL_CATALOG";

/// How often the overlay file is checked for a change. Pricing asks for the
/// table once per token-usage row, so checking on every call would be a
/// `stat` per row of a dashboard.
const RECHECK: std::time::Duration = std::time::Duration::from_secs(30);

/// The provider ids an overlay row may name: the agents the harness can run.
pub const PROVIDERS: &[&str] = &["claude", "codex", "pi", "cursor", "anthropic-api"];

/// The table in force: the bundled one, with the overlay merged over it.
#[derive(Debug, Clone)]
pub struct Effective {
    pub families: Vec<Family>,
    pub models: Vec<Model>,
}

impl Effective {
    fn bundled() -> Self {
        Self {
            families: FAMILIES.to_vec(),
            models: MODELS.to_vec(),
        }
    }

    /// A family by id. Panics on an unknown one, which can only be a typo in
    /// [`MODELS`] — a test walks every row — since an overlay naming an
    /// unknown family is rejected before it is merged.
    fn family(&self, id: &str) -> Family {
        self.families
            .iter()
            .find(|f| f.id == id)
            .copied()
            .unwrap_or_else(|| panic!("no pricing family `{id}`"))
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct OverlayFile {
    #[serde(default)]
    families: Vec<OverlayFamily>,
    #[serde(default)]
    models: Vec<OverlayModel>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct OverlayFamily {
    id: String,
    lane: String,
    rates: OverlayRates,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct OverlayRates {
    input: f64,
    output: f64,
    cache_read: f64,
    cache_write: f64,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct OverlayModel {
    provider: String,
    id: String,
    family: String,
    #[serde(default)]
    included: bool,
    #[serde(default)]
    min_cli: Option<String>,
}

/// Merge an overlay over the bundled table.
///
/// All or nothing: every problem in the file is collected and any one rejects
/// the whole overlay, because half an overlay is a table nobody wrote. A family
/// or model with the id of a bundled one replaces it in place; a new one is
/// added after the bundled rows.
pub fn merge_overlay(json: &str) -> Result<Effective, Vec<String>> {
    let file: OverlayFile =
        serde_json::from_str(json).map_err(|e| vec![format!("not a valid catalog: {e}")])?;
    let mut errors = Vec::new();

    let mut family_ids: std::collections::HashSet<&str> = FAMILIES.iter().map(|f| f.id).collect();
    let mut seen = std::collections::HashSet::new();
    for f in &file.families {
        if f.id.trim().is_empty() {
            errors.push("a family has an empty `id`".to_string());
            continue;
        }
        if !seen.insert(f.id.as_str()) {
            errors.push(format!("family `{}` is listed twice", f.id));
        }
        if f.lane.trim().is_empty() {
            errors.push(format!("family `{}` has an empty `lane`", f.id));
        }
        let r = &f.rates;
        for (name, v) in [
            ("input", r.input),
            ("output", r.output),
            ("cache_read", r.cache_read),
            ("cache_write", r.cache_write),
        ] {
            if !v.is_finite() || v < 0.0 {
                errors.push(format!("family `{}` has rate `{name}` = {v}", f.id));
            }
        }
        family_ids.insert(f.id.as_str());
    }

    let mut seen = std::collections::HashSet::new();
    for m in &file.models {
        if m.id.trim().is_empty() {
            errors.push(format!("a `{}` model has an empty `id`", m.provider));
            continue;
        }
        if !PROVIDERS.contains(&m.provider.as_str()) {
            errors.push(format!(
                "model `{}` names provider `{}`; expected one of {}",
                m.id,
                m.provider,
                PROVIDERS.join(", ")
            ));
        }
        if !seen.insert((m.provider.as_str(), m.id.as_str())) {
            errors.push(format!(
                "model `{}` is listed twice for `{}`",
                m.id, m.provider
            ));
        }
        if !family_ids.contains(m.family.as_str()) {
            errors.push(format!(
                "model `{}` names family `{}`, which neither the bundled table nor the overlay defines",
                m.id, m.family
            ));
        }
        if let Some(v) = &m.min_cli {
            if parse_version(v).is_none() {
                errors.push(format!(
                    "model `{}` has `min_cli` `{v}`, which is not a version",
                    m.id
                ));
            }
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    let mut table = Effective::bundled();
    for f in file.families {
        let family = Family {
            id: leak(f.id),
            lane: leak(f.lane),
            rates: rates(
                f.rates.input,
                f.rates.output,
                f.rates.cache_read,
                f.rates.cache_write,
            ),
        };
        match table.families.iter_mut().find(|x| x.id == family.id) {
            Some(slot) => *slot = family,
            None => table.families.push(family),
        }
    }
    for m in file.models {
        let model = Model {
            provider: leak(m.provider),
            id: leak(m.id),
            family: leak(m.family),
            included: m.included,
            min_cli: m.min_cli.map(leak),
        };
        match table
            .models
            .iter_mut()
            .find(|x| x.provider == model.provider && x.id == model.id)
        {
            Some(slot) => *slot = model,
            None => table.models.push(model),
        }
    }
    Ok(table)
}

/// Give an overlay string the `'static` lifetime the table's rows have.
///
/// Leaks, deliberately: the overlay is merged only when its file changes, so
/// this costs a few kilobytes per edit, and it keeps every row the same plain
/// `Copy` type whether it shipped in the binary or arrived in the file.
fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

struct Cache {
    checked: Option<std::time::Instant>,
    path: Option<std::path::PathBuf>,
    modified: Option<std::time::SystemTime>,
    table: std::sync::Arc<Effective>,
}

static CACHE: std::sync::LazyLock<std::sync::Mutex<Cache>> = std::sync::LazyLock::new(|| {
    std::sync::Mutex::new(Cache {
        checked: None,
        path: None,
        modified: None,
        table: std::sync::Arc::new(Effective::bundled()),
    })
});

/// The table in force now: bundled, plus the overlay when one is configured
/// and valid. Re-reads the overlay when its file changes, at most every
/// [`RECHECK`].
pub fn effective() -> std::sync::Arc<Effective> {
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if cache.checked.is_some_and(|t| t.elapsed() < RECHECK) {
        return cache.table.clone();
    }
    cache.checked = Some(std::time::Instant::now());
    let path = overlay_path();
    let modified = path
        .as_ref()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok());
    if path != cache.path || modified != cache.modified {
        cache.table = std::sync::Arc::new(load(path.as_deref()));
        cache.path = path;
        cache.modified = modified;
    }
    cache.table.clone()
}

fn overlay_path() -> Option<std::path::PathBuf> {
    if let Some(p) = std::env::var_os(CATALOG_ENV).filter(|p| !p.is_empty()) {
        return Some(p.into());
    }
    let p =
        std::path::PathBuf::from(std::env::var_os("HARNESS_DATA_DIR")?).join("model-catalog.json");
    p.is_file().then_some(p)
}

fn load(path: Option<&std::path::Path>) -> Effective {
    let Some(path) = path else {
        return Effective::bundled();
    };
    let json = match std::fs::read_to_string(path) {
        Ok(json) => json,
        Err(e) => {
            tracing::warn!(path = %path.display(), "model catalog overlay unreadable, using the bundled table: {e}");
            return Effective::bundled();
        }
    };
    match merge_overlay(&json) {
        Ok(table) => {
            tracing::info!(path = %path.display(), models = table.models.len(), "model catalog overlay loaded");
            table
        }
        Err(errors) => {
            tracing::error!(
                path = %path.display(),
                "model catalog overlay rejected, using the bundled table: {}",
                errors.join("; ")
            );
            Effective::bundled()
        }
    }
}

/// Whether `installed` is at least `min`. Compares dotted numbers
/// (`18.1.12`), ignoring a leading `v` and anything after a part's digits
/// (`-beta`). An unreadable version on either side counts as enough: a model
/// is only held back when the installed CLI is known to be too old.
pub fn meets_min_cli(installed: &str, min: &str) -> bool {
    match (parse_version(installed), parse_version(min)) {
        (Some(have), Some(need)) => {
            let len = have.len().max(need.len());
            let part = |v: &[u64], i: usize| v.get(i).copied().unwrap_or(0);
            (0..len)
                .map(|i| part(&have, i).cmp(&part(&need, i)))
                .find(|o| o.is_ne())
                .is_none_or(|o| o.is_gt())
        }
        _ => true,
    }
}

fn parse_version(v: &str) -> Option<Vec<u64>> {
    let parts: Vec<u64> = v
        .trim()
        .trim_start_matches('v')
        .split('.')
        .map_while(|p| {
            let digits: String = p.chars().take_while(char::is_ascii_digit).collect();
            digits.parse().ok()
        })
        .collect();
    (!parts.is_empty()).then_some(parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every row has to price. A typo in a `family` would otherwise panic on a
    /// running server the first time somebody used that model.
    #[test]
    fn every_model_names_a_family_that_exists() {
        for m in MODELS {
            assert!(
                FAMILIES.iter().any(|f| f.id == m.family),
                "`{}` on `{}` names unknown family `{}`",
                m.id,
                m.provider,
                m.family
            );
        }
    }

    /// The same model reached through two agents must cost the same, or an A/B
    /// between them measures the table rather than the models.
    #[test]
    fn the_same_model_prices_the_same_whichever_agent_reaches_it() {
        // Codex's bare id and omp's prefixed one are one model.
        assert_eq!(
            family_for("gpt-6-astra").id,
            family_for("openai-codex/gpt-6-astra").id
        );
        assert_eq!(
            family_for("gpt-5.6-luna").id,
            family_for("openai-codex/gpt-5.6-luna").id
        );
        // Claude Code's alias and Cursor's versioned id are one model.
        assert_eq!(family_for("sonnet").id, family_for("claude-sonnet-5").id);
        assert_eq!(family_for("opus").id, family_for("claude-opus-5").id);
    }

    /// The tiers are 25x apart, so a fallback that lumped them together would
    /// quietly make Luna look as expensive as Sol.
    #[test]
    fn the_gpt_tiers_do_not_collapse_into_one_price() {
        let sol = family_for("gpt-5.6-sol").rates;
        let terra = family_for("gpt-5.6-terra").rates;
        let luna = family_for("gpt-5.6-luna").rates;
        assert!(sol.output > terra.output && terra.output > luna.output);
        // Astra is priced above Sol rather than folded into the gpt-5 arm it
        // also matches by substring.
        assert!(family_for("gpt-6-astra").rates.output > sol.output);
    }

    /// A model nobody listed still has to price — a workflow can pin any id.
    #[test]
    fn an_unlisted_model_still_prices_by_shape() {
        assert_eq!(family_for("claude-opus-4.8").id, "opus");
        assert_eq!(family_for("openai-codex/gpt-5.9").id, "gpt-5");
        assert_eq!(family_for("kimi-code/kimi-k3").id, "kimi");
        // And something entirely unknown falls to the Sonnet tier rather than
        // to zero, which would read as a free run.
        assert_eq!(family_for("who-knows-1").id, "sonnet");
    }

    /// The editor lists what each agent can actually be handed. omp needs the
    /// backend prefix; Codex must not have one.
    #[test]
    fn each_provider_lists_ids_in_its_own_shape() {
        assert!(models_for("codex").contains(&"gpt-6-astra"));
        assert!(models_for("pi").contains(&"openai-codex/gpt-6-astra"));
        assert!(!models_for("pi").iter().any(|id| !id.contains('/')));
        assert!(!models_for("codex").iter().any(|id| id.contains('/')));
        assert!(models_for("nobody").is_empty());
    }

    /// The web prices runs too, and it reads a generated copy of this table
    /// rather than a second hand-written one. This test is what keeps the copy
    /// honest: edit the table, and it fails until the JSON is regenerated.
    ///
    /// Regenerate with `UPDATE_MODEL_CATALOG=1 cargo test -p harness-runner`.
    #[test]
    fn the_generated_catalog_matches_this_table() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../web/src/lib/model-catalog.json");
        let current = catalog_json();
        if std::env::var("UPDATE_MODEL_CATALOG").is_ok() {
            std::fs::write(&path, format!("{current}\n")).expect("write catalog");
            return;
        }
        // Compare content, not line endings. Git checks this file out with CRLF
        // on Windows (`core.autocrlf`), while the generator writes LF — so a raw
        // comparison failed on every Windows clone and passed on CI, which is
        // the worst way round for a guard nobody would suspect of being wrong.
        let on_disk = std::fs::read_to_string(&path)
            .unwrap_or_default()
            .replace("\r\n", "\n");
        assert_eq!(
            on_disk.trim(),
            current.trim(),
            "web/src/lib/model-catalog.json is stale — regenerate with \
             UPDATE_MODEL_CATALOG=1 cargo test -p harness-runner"
        );
    }

    /// Ordering is the whole subtlety of the fallbacks: the specific tiers have
    /// to be reached before the general arm that also matches them.
    #[test]
    fn a_specific_tier_is_matched_before_the_general_arm() {
        // Both contain "gpt-5", and one contains "codex" too.
        assert_eq!(family_for("some-gpt-5.6-luna-build").id, "gpt-5.6-luna");
        assert_eq!(family_for("gpt-6-astra-codex").id, "gpt-6");
        // Cursor's `claude-sonnet-5` is listed, so it never reaches a fallback
        // — but an unlisted Anthropic id must still find Sonnet, not gpt-5.
        assert_eq!(family_for("claude-sonnet-4.9").id, "sonnet");
    }

    const OVERLAY: &str = r#"{
        "families": [
            { "id": "opus-6", "lane": "claude",
              "rates": { "input": 6.0, "output": 30.0, "cache_read": 0.6, "cache_write": 7.5 } }
        ],
        "models": [
            { "provider": "claude", "id": "claude-opus-6", "family": "opus-6", "included": true },
            { "provider": "pi", "id": "kimi-code/kimi-k3", "family": "kimi", "included": true,
              "min_cli": "18.2.0" },
            { "provider": "cursor", "id": "composer-2.5", "family": "grok", "included": false }
        ]
    }"#;

    #[test]
    fn an_overlay_adds_models_and_families_without_a_release() {
        let table = merge_overlay(OVERLAY).expect("valid overlay");
        let opus6 = table
            .models
            .iter()
            .find(|m| m.id == "claude-opus-6")
            .expect("new model listed");
        assert_eq!(opus6.provider, "claude");
        assert_eq!(table.family(opus6.family).rates.output, 30.0);
        let k3 = table
            .models
            .iter()
            .find(|m| m.id == "kimi-code/kimi-k3")
            .unwrap();
        assert_eq!(k3.min_cli, Some("18.2.0"));
        // Every bundled row is still there.
        assert_eq!(table.models.len(), MODELS.len() + 2);
    }

    #[test]
    fn an_overlay_row_with_a_bundled_id_replaces_it_in_place() {
        let table = merge_overlay(OVERLAY).unwrap();
        let rows: Vec<&Model> = table
            .models
            .iter()
            .filter(|m| m.provider == "cursor" && m.id == "composer-2.5")
            .collect();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].family, "grok");
        assert!(!rows[0].included);
        // Still first among Cursor's models, where the bundled row was.
        let first_cursor = table
            .models
            .iter()
            .find(|m| m.provider == "cursor")
            .unwrap();
        assert_eq!(first_cursor.id, "composer-2.5");
    }

    #[test]
    fn a_broken_overlay_is_rejected_whole_with_every_reason() {
        let errors = merge_overlay(
            r#"{
            "families": [
                { "id": "cheap", "lane": "", "rates": { "input": -1, "output": 1, "cache_read": 0, "cache_write": 0 } }
            ],
            "models": [
                { "provider": "claude", "id": "fine", "family": "sonnet" },
                { "provider": "gemini-cli", "id": "g", "family": "gemini" },
                { "provider": "pi", "id": "x", "family": "nope" },
                { "provider": "pi", "id": "y", "family": "kimi", "min_cli": "latest" }
            ]
        }"#,
        )
        .expect_err("overlay has errors");
        let all = errors.join("\n");
        assert!(all.contains("empty `lane`"), "{all}");
        assert!(all.contains("rate `input` = -1"), "{all}");
        assert!(all.contains("provider `gemini-cli`"), "{all}");
        assert!(all.contains("family `nope`"), "{all}");
        assert!(all.contains("`latest`, which is not a version"), "{all}");
        assert_eq!(errors.len(), 5, "{all}");
    }

    #[test]
    fn an_overlay_that_is_not_a_catalog_is_rejected() {
        assert!(merge_overlay("not json").is_err());
        // A misspelt field is a mistake worth reporting, not a field to ignore.
        let errors =
            merge_overlay(r#"{"models": [{"provider": "claude", "id": "a", "familly": "opus"}]}"#)
                .expect_err("unknown field");
        assert!(errors[0].contains("familly"), "{errors:?}");
    }

    #[test]
    fn loading_falls_back_to_the_bundled_table() {
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("good.json");
        let bad = dir.path().join("bad.json");
        std::fs::write(&good, OVERLAY).unwrap();
        std::fs::write(
            &bad,
            r#"{"models": [{"provider": "nobody", "id": "a", "family": "opus"}]}"#,
        )
        .unwrap();

        assert_eq!(load(Some(&good)).models.len(), MODELS.len() + 2);
        assert_eq!(load(Some(&bad)).models.len(), MODELS.len());
        assert_eq!(
            load(Some(&dir.path().join("missing.json"))).models.len(),
            MODELS.len()
        );
        assert_eq!(load(None).models.len(), MODELS.len());
    }

    #[test]
    fn min_cli_compares_versions_by_number() {
        assert!(meets_min_cli("18.1.12", "18.1.12"));
        assert!(meets_min_cli("18.1.15", "18.1.12"));
        assert!(meets_min_cli("18.10.0", "18.9.9"));
        assert!(meets_min_cli("v2.1.316", "2.1"));
        assert!(!meets_min_cli("18.1.9", "18.1.12"));
        assert!(!meets_min_cli("2.0", "2.0.1"));
        assert!(meets_min_cli("18.2.0-beta.1", "18.2.0"));
        // Unknown either side: do not hold the model back.
        assert!(meets_min_cli("unknown", "18.1.12"));
    }

    #[test]
    fn the_bundled_minimums_are_versions() {
        for m in MODELS {
            if let Some(v) = m.min_cli {
                assert!(parse_version(v).is_some(), "`{}` min_cli `{v}`", m.id);
            }
        }
    }

    /// Cursor's own models are included; Claude and GPT through Cursor are
    /// metered at API prices. Listing them without that distinction is how
    /// somebody picks an Opus node expecting it to be covered.
    #[test]
    fn cursor_records_which_models_the_subscription_covers() {
        let included = |id: &str| {
            MODELS
                .iter()
                .find(|m| m.provider == "cursor" && m.id == id)
                .unwrap_or_else(|| panic!("no cursor model {id}"))
                .included
        };
        assert!(included("composer-2.5"));
        assert!(included("grok-4.6"));
        assert!(!included("claude-opus-5"));
        assert!(!included("gpt-5.6-sol"));
    }
}
