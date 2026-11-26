//! **Skills and prompts served over `/mcp`.** The harness tells a connected
//! assistant how to use its own tools, so every editor gets the same guidance
//! without a copy of a skill in each repository.
//!
//! - **Skills** (`io.modelcontextprotocol/skills`, SEP-2640): `skills/list`,
//!   `skills/get`, and their files through `resources/read`. Clients only ask
//!   for them on the stateless 2026-07-28 protocol, which is why
//!   [`super::mcp_routes`] offers the two methods to modern requests only.
//! - **Resources**: the same files under `resources/list` / `resources/read`,
//!   for any client.
//! - **Prompts**: `diagnose-run`, which an editor shows as a slash command.
//!   Its text is the `diagnose-runs` skill's, so the two cannot drift.
//!
//! The files are compiled in. `author-workflows` serves the user docs' own
//! authoring reference as `reference/authoring.md`, so the skill and the
//! published docs are one text.

use std::sync::LazyLock;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// The extension id the server declares in its capabilities.
pub(crate) const SKILLS_EXTENSION: &str = "io.modelcontextprotocol/skills";

/// JSON-RPC "invalid params": what an unknown skill, file or prompt gets.
pub(crate) const INVALID_PARAMS: i32 = -32602;

/// Every URI this server serves starts here. The first segment is the URI
/// authority, and the last segment of a skill's path is its name.
const URI_ROOT: &str = "skill://harness";

struct SourceFile {
    path: &'static str,
    mime: &'static str,
    text: &'static str,
}

struct Source {
    name: &'static str,
    files: &'static [SourceFile],
}

const SOURCES: &[Source] = &[
    Source {
        name: "author-workflows",
        files: &[
            SourceFile {
                path: "SKILL.md",
                mime: "text/markdown",
                text: include_str!("../../mcp-skills/author-workflows/SKILL.md"),
            },
            SourceFile {
                path: "reference/authoring.md",
                mime: "text/markdown",
                text: include_str!("../../../../site/content/docs/workflows/authoring.mdx"),
            },
        ],
    },
    Source {
        name: "diagnose-runs",
        files: &[SourceFile {
            path: "SKILL.md",
            mime: "text/markdown",
            text: include_str!("../../mcp-skills/diagnose-runs/SKILL.md"),
        }],
    },
];

struct ServedFile {
    uri: String,
    mime: &'static str,
    text: &'static str,
    digest: String,
}

struct Skill {
    name: &'static str,
    /// The SKILL.md frontmatter, as the listing must carry it.
    frontmatter: Value,
    /// SKILL.md without its frontmatter.
    body: &'static str,
    /// SKILL.md first.
    files: Vec<ServedFile>,
}

static SKILLS: LazyLock<Vec<Skill>> = LazyLock::new(|| {
    SOURCES
        .iter()
        .map(|s| build(s).unwrap_or_else(|e| panic!("bundled skill {}: {e}", s.name)))
        .collect()
});

fn build(source: &Source) -> Result<Skill, String> {
    let skill_md = source
        .files
        .first()
        .filter(|f| f.path == "SKILL.md")
        .ok_or("the first file must be SKILL.md")?;
    let (frontmatter, body) = split_frontmatter(skill_md.text)?;
    let frontmatter: Value =
        serde_yaml::from_str(frontmatter).map_err(|e| format!("frontmatter: {e}"))?;
    if frontmatter.get("name").and_then(Value::as_str) != Some(source.name) {
        return Err("frontmatter `name` must equal the skill's folder name".into());
    }
    if frontmatter
        .get("description")
        .and_then(Value::as_str)
        .is_none_or(|d| d.trim().is_empty())
    {
        return Err("frontmatter needs a `description`".into());
    }
    let files = source
        .files
        .iter()
        .map(|f| ServedFile {
            uri: format!("{URI_ROOT}/{}/{}", source.name, f.path),
            mime: f.mime,
            text: f.text,
            digest: format!("sha256:{:x}", Sha256::digest(f.text.as_bytes())),
        })
        .collect();
    Ok(Skill {
        name: source.name,
        frontmatter,
        body,
        files,
    })
}

/// `---\n<yaml>\n---\n<body>` → (yaml, body).
fn split_frontmatter(text: &str) -> Result<(&str, &str), String> {
    let rest = text
        .strip_prefix("---\n")
        .ok_or("SKILL.md must open with `---` frontmatter")?;
    let end = rest
        .find("\n---\n")
        .ok_or("SKILL.md frontmatter is not closed with `---`")?;
    Ok((&rest[..end], rest[end + "\n---\n".len()..].trim_start()))
}

fn skill_entry(skill: &Skill) -> Value {
    let resources: Vec<Value> = skill
        .files
        .iter()
        .map(|f| json!({ "uri": f.uri, "digest": f.digest, "size": f.text.len() }))
        .collect();
    json!({
        "uri": skill.files[0].uri,
        "frontmatter": skill.frontmatter,
        "resources": resources,
    })
}

fn uri_param(params: &Value) -> Result<&str, (i32, String)> {
    params
        .get("uri")
        .and_then(Value::as_str)
        .ok_or_else(|| (INVALID_PARAMS, "`uri` is required".to_string()))
}

/// `skills/list`: every skill, in one page.
pub(crate) fn skills_list() -> Value {
    json!({ "skills": SKILLS.iter().map(skill_entry).collect::<Vec<_>>() })
}

/// `skills/get`: one skill, by the URI of its SKILL.md.
pub(crate) fn skills_get(params: &Value) -> Result<Value, (i32, String)> {
    let uri = uri_param(params)?;
    SKILLS
        .iter()
        .find(|s| s.files[0].uri == uri)
        .map(|s| json!({ "skill": skill_entry(s) }))
        .ok_or_else(|| (INVALID_PARAMS, format!("No skill is served at {uri}")))
}

/// `resources/list`: every skill file.
pub(crate) fn resources_list() -> Value {
    let mut resources = Vec::new();
    for skill in SKILLS.iter() {
        for (i, f) in skill.files.iter().enumerate() {
            let mut entry = json!({ "uri": f.uri, "mimeType": f.mime });
            if i == 0 {
                entry["name"] = json!(skill.name);
                entry["description"] = skill.frontmatter["description"].clone();
            } else {
                entry["name"] = json!(f.uri.rsplit('/').next().unwrap_or(&f.uri));
            }
            resources.push(entry);
        }
    }
    json!({ "resources": resources })
}

/// `resources/read`: one skill file.
pub(crate) fn resources_read(params: &Value) -> Result<Value, (i32, String)> {
    let uri = uri_param(params)?;
    SKILLS
        .iter()
        .flat_map(|s| s.files.iter())
        .find(|f| f.uri == uri)
        .map(|f| json!({ "contents": [{ "uri": f.uri, "mimeType": f.mime, "text": f.text }] }))
        .ok_or_else(|| (INVALID_PARAMS, format!("No resource is served at {uri}")))
}

const DIAGNOSE_RUN: &str = "diagnose-run";

/// Longest run id the prompt accepts. Real ids are a workflow name and a
/// timestamp; this only stops a pasted page from becoming the prompt.
const MAX_RUN_ID: usize = 200;

/// `prompts/list`.
pub(crate) fn prompts_list() -> Value {
    json!({ "prompts": [{
        "name": DIAGNOSE_RUN,
        "title": "Diagnose a run",
        "description": "Find out what happened in a workflow run and why: which step failed, \
                        was skipped or was cancelled, and what to change.",
        "arguments": [{
            "name": "run_id",
            "description": "The run's id, e.g. task-1794194165697",
            "required": true,
        }],
    }] })
}

/// `prompts/get`.
pub(crate) fn prompts_get(params: &Value) -> Result<Value, (i32, String)> {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    if name != DIAGNOSE_RUN {
        return Err((INVALID_PARAMS, format!("No prompt named `{name}`")));
    }
    let run_id = params
        .pointer("/arguments/run_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    if run_id.is_empty() || run_id.len() > MAX_RUN_ID {
        return Err((
            INVALID_PARAMS,
            "`run_id` is required, e.g. task-1794194165697".into(),
        ));
    }
    let guide = SKILLS
        .iter()
        .find(|s| s.name == "diagnose-runs")
        .map(|s| s.body)
        .unwrap_or_default();
    Ok(json!({
        "description": format!("Diagnose run {run_id}"),
        "messages": [{
            "role": "user",
            "content": {
                "type": "text",
                "text": format!(
                    "Diagnose harness run `{run_id}`: what happened, in which step, and why, \
                     and what to change. Follow this guide.\n\n{guide}"
                ),
            },
        }],
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bundled_skill_builds() {
        // Forces the LazyLock: a bad frontmatter or a name that does not match
        // its folder fails here, not on the first client that asks.
        let names: Vec<&str> = SKILLS.iter().map(|s| s.name).collect();
        assert_eq!(names, ["author-workflows", "diagnose-runs"]);
    }

    #[test]
    fn listing_carries_a_complete_manifest() {
        let list = skills_list();
        for skill in list["skills"].as_array().unwrap() {
            let name = skill["frontmatter"]["name"].as_str().unwrap();
            let uri = skill["uri"].as_str().unwrap();
            // The skill path's last segment is the skill's name.
            assert_eq!(uri, format!("skill://harness/{name}/SKILL.md"));
            let resources = skill["resources"].as_array().unwrap();
            assert_eq!(resources[0]["uri"], uri, "SKILL.md is listed too");
            for r in resources {
                let digest = r["digest"].as_str().unwrap();
                let hex = digest.strip_prefix("sha256:").unwrap();
                assert_eq!(hex.len(), 64);
                assert!(hex
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
                // The listed size is the size of what resources/read returns.
                let read = resources_read(&json!({ "uri": r["uri"] })).unwrap();
                let text = read["contents"][0]["text"].as_str().unwrap();
                assert_eq!(r["size"], text.len());
                assert_eq!(
                    digest,
                    format!("sha256:{:x}", Sha256::digest(text.as_bytes()))
                );
            }
        }
    }

    #[test]
    fn authoring_reference_is_the_published_doc() {
        let read = resources_read(
            &json!({ "uri": "skill://harness/author-workflows/reference/authoring.md" }),
        )
        .unwrap();
        let text = read["contents"][0]["text"].as_str().unwrap();
        assert!(text.contains("title: \"Authoring workflows\""));
    }

    #[test]
    fn get_by_uri_and_unknown_uris() {
        let got = skills_get(&json!({ "uri": "skill://harness/diagnose-runs/SKILL.md" })).unwrap();
        assert_eq!(got["skill"]["frontmatter"]["name"], "diagnose-runs");

        let (code, msg) =
            skills_get(&json!({ "uri": "skill://harness/nope/SKILL.md" })).unwrap_err();
        assert_eq!(code, INVALID_PARAMS);
        assert!(msg.contains("skill://harness/nope/SKILL.md"));
        assert_eq!(skills_get(&json!({})).unwrap_err().0, INVALID_PARAMS);
        assert_eq!(
            resources_read(&json!({ "uri": "skill://harness/diagnose-runs/x.md" }))
                .unwrap_err()
                .0,
            INVALID_PARAMS
        );
    }

    #[test]
    fn resources_list_names_each_file() {
        let list = resources_list();
        let all = list["resources"].as_array().unwrap();
        assert_eq!(all.len(), 3);
        let skill = all
            .iter()
            .find(|r| r["uri"] == "skill://harness/diagnose-runs/SKILL.md")
            .unwrap();
        assert_eq!(skill["name"], "diagnose-runs");
        assert!(skill["description"].as_str().unwrap().contains("run"));
        let reference = all.iter().find(|r| r["name"] == "authoring.md").unwrap();
        assert_eq!(reference["mimeType"], "text/markdown");
    }

    #[test]
    fn diagnose_run_prompt_carries_the_skill_text() {
        let listed = prompts_list();
        assert_eq!(listed["prompts"][0]["name"], "diagnose-run");
        assert_eq!(listed["prompts"][0]["arguments"][0]["required"], true);

        let got = prompts_get(&json!({
            "name": "diagnose-run",
            "arguments": { "run_id": " security-audit-1795651221487 " },
        }))
        .unwrap();
        let text = got["messages"][0]["content"]["text"].as_str().unwrap();
        assert!(text.starts_with("Diagnose harness run `security-audit-1795651221487`"));
        assert!(text.contains("# Diagnosing a harness run"));
        assert!(
            !text.contains("name: diagnose-runs"),
            "frontmatter is not part of the prompt"
        );

        assert_eq!(
            prompts_get(&json!({ "name": "diagnose-run", "arguments": {} }))
                .unwrap_err()
                .0,
            INVALID_PARAMS
        );
        assert_eq!(
            prompts_get(&json!({ "name": "other" })).unwrap_err().0,
            INVALID_PARAMS
        );
    }

    #[test]
    fn frontmatter_must_open_and_close() {
        assert!(split_frontmatter("no frontmatter").is_err());
        assert!(split_frontmatter("---\nname: x\n").is_err());
        let (fm, body) = split_frontmatter("---\nname: x\n---\n\n# Body\n").unwrap();
        assert_eq!(fm, "name: x");
        assert_eq!(body, "# Body\n");
    }
}
