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
    },
    Model {
        provider: "claude",
        id: "opus",
        family: "opus",
        included: true,
    },
    Model {
        provider: "claude",
        id: "haiku",
        family: "haiku",
        included: true,
    },
    Model {
        provider: "claude",
        id: "fable",
        family: "fable",
        included: true,
    },
    // ── Codex CLI on a ChatGPT account ──────────────────────────────────────
    // The general models, not the `-codex` variants, which need API-key auth.
    // Versioned because Codex publishes no alias for these tiers.
    Model {
        provider: "codex",
        id: "gpt-6-astra",
        family: "gpt-6",
        included: true,
    },
    Model {
        provider: "codex",
        id: "gpt-5.6-sol",
        family: "gpt-5.6-sol",
        included: true,
    },
    Model {
        provider: "codex",
        id: "gpt-5.6-terra",
        family: "gpt-5.6-terra",
        included: true,
    },
    Model {
        provider: "codex",
        id: "gpt-5.6-luna",
        family: "gpt-5.6-luna",
        included: true,
    },
    Model {
        provider: "codex",
        id: "gpt-5.5",
        family: "gpt-5",
        included: true,
    },
    Model {
        provider: "codex",
        id: "gpt-5.4",
        family: "gpt-5",
        included: true,
    },
    Model {
        provider: "codex",
        id: "gpt-5.4-mini",
        family: "gpt-5",
        included: true,
    },
    // ── omp (`pi`) ──────────────────────────────────────────────────────────
    // omp rejects a bare OpenAI id — `resolve_model` mis-prefixes it to
    // `kimi-code/` — so these carry the backend prefix.
    Model {
        provider: "pi",
        id: "openai-codex/gpt-6-astra",
        family: "gpt-6",
        included: true,
    },
    Model {
        provider: "pi",
        id: "openai-codex/gpt-5.6-sol",
        family: "gpt-5.6-sol",
        included: true,
    },
    Model {
        provider: "pi",
        id: "openai-codex/gpt-5.6-terra",
        family: "gpt-5.6-terra",
        included: true,
    },
    Model {
        provider: "pi",
        id: "openai-codex/gpt-5.6-luna",
        family: "gpt-5.6-luna",
        included: true,
    },
    Model {
        provider: "pi",
        id: "openai-codex/gpt-5.5",
        family: "gpt-5",
        included: true,
    },
    Model {
        provider: "pi",
        id: "openai-codex/gpt-5.4-nano",
        family: "gpt-5",
        included: true,
    },
    Model {
        provider: "pi",
        id: "openai-codex/gpt-5.2-codex",
        family: "gpt-5",
        included: true,
    },
    Model {
        provider: "pi",
        id: "openai-codex/gpt-5.1-codex-max",
        family: "gpt-5",
        included: true,
    },
    Model {
        provider: "pi",
        id: "openai-codex/gpt-5.1-codex",
        family: "gpt-5",
        included: true,
    },
    Model {
        provider: "pi",
        id: "kimi-code/kimi-for-coding",
        family: "kimi",
        included: true,
    },
    Model {
        provider: "pi",
        id: "kimi-code/kimi-k2",
        family: "kimi",
        included: true,
    },
    Model {
        provider: "pi",
        id: "kimi-code/kimi-k2-turbo-preview",
        family: "kimi",
        included: true,
    },
    Model {
        provider: "pi",
        id: "kimi-code/kimi-k2.5",
        family: "kimi",
        included: true,
    },
    // ── Cursor ──────────────────────────────────────────────────────────────
    // Cursor's own models are included in the subscription; everything below
    // them draws from the "Other Models" pool at the model's API price.
    Model {
        provider: "cursor",
        id: "composer-2.5",
        family: "composer",
        included: true,
    },
    Model {
        provider: "cursor",
        id: "composer-2.5-fast",
        family: "composer",
        included: true,
    },
    Model {
        provider: "cursor",
        id: "grok-4.6",
        family: "grok",
        included: true,
    },
    Model {
        provider: "cursor",
        id: "grok-4.6-fast",
        family: "grok",
        included: true,
    },
    Model {
        provider: "cursor",
        id: "claude-sonnet-5",
        family: "sonnet",
        included: false,
    },
    Model {
        provider: "cursor",
        id: "claude-opus-5",
        family: "opus",
        included: false,
    },
    Model {
        provider: "cursor",
        id: "claude-fable-5.1",
        family: "fable",
        included: false,
    },
    Model {
        provider: "cursor",
        id: "claude-4.5-haiku",
        family: "haiku",
        included: false,
    },
    Model {
        provider: "cursor",
        id: "gpt-5.6-sol",
        family: "gpt-5.6-sol",
        included: false,
    },
    Model {
        provider: "cursor",
        id: "gpt-5.6-terra",
        family: "gpt-5.6-terra",
        included: false,
    },
    Model {
        provider: "cursor",
        id: "gpt-5.6-luna",
        family: "gpt-5.6-luna",
        included: false,
    },
    Model {
        provider: "cursor",
        id: "gpt-5.5",
        family: "gpt-5",
        included: false,
    },
    Model {
        provider: "cursor",
        id: "gemini-3-pro",
        family: "gemini",
        included: false,
    },
    // ── Direct Anthropic API (API key, not a subscription) ──────────────────
    Model {
        provider: "anthropic-api",
        id: "sonnet",
        family: "sonnet",
        included: false,
    },
    Model {
        provider: "anthropic-api",
        id: "opus",
        family: "opus",
        included: false,
    },
];

/// The ids one provider offers, in listing order.
pub fn models_for(provider: &str) -> Vec<&'static str> {
    MODELS
        .iter()
        .filter(|m| m.provider == provider)
        .map(|m| m.id)
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
pub fn family_for(model: &str) -> &'static Family {
    if let Some(m) = MODELS.iter().find(|m| m.id.eq_ignore_ascii_case(model)) {
        return family(m.family);
    }
    let lower = model.to_ascii_lowercase();
    let id = FALLBACKS
        .iter()
        .find(|(needle, _)| lower.contains(needle))
        .map(|(_, family)| *family)
        .unwrap_or(DEFAULT_FAMILY);
    family(id)
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

/// A family by id. Panics on an unknown one, which can only be a typo in
/// [`MODELS`] — and a test walks every row so it cannot reach a running server.
fn family(id: &str) -> &'static Family {
    FAMILIES
        .iter()
        .find(|f| f.id == id)
        .unwrap_or_else(|| panic!("no pricing family `{id}`"))
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
        let on_disk = std::fs::read_to_string(&path).unwrap_or_default();
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
