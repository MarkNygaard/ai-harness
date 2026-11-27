//! `harness grafana …` — read the connected Grafana from a workflow step.
//!
//! Asks the server's run proxy (`/api/run/grafana/*`, see
//! `http::run_grafana_routes`) with the run's grant, like `harness linear`
//! does (see [`crate::linear_cli`]). The step never holds the Grafana token.
//!
//! `get` reads any path the proxy allows; `loki` and `prometheus` run one
//! query, finding the datasource themselves.

use serde::Deserialize;

/// Which kind of datasource a query is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryKind {
    Loki,
    Prometheus,
}

impl QueryKind {
    fn datasource_type(self) -> &'static str {
        match self {
            QueryKind::Loki => "loki",
            QueryKind::Prometheus => "prometheus",
        }
    }

    /// The datasource's own API under Grafana's proxy.
    pub(crate) fn api(self, range: bool) -> &'static str {
        match (self, range) {
            (QueryKind::Loki, false) => "loki/api/v1/query",
            (QueryKind::Loki, true) => "loki/api/v1/query_range",
            (QueryKind::Prometheus, false) => "api/v1/query",
            (QueryKind::Prometheus, true) => "api/v1/query_range",
        }
    }
}

/// One LogQL or PromQL query. Times are what the datasource accepts: Unix
/// seconds or RFC 3339.
#[derive(Debug, Default)]
pub struct Query {
    pub expr: String,
    /// An instant query at this time (default: now).
    pub time: Option<String>,
    /// A range query from `start` to `end`.
    pub start: Option<String>,
    pub end: Option<String>,
    /// The range query's resolution, e.g. `1h` or `3600`.
    pub step: Option<String>,
    /// Loki: the most log lines to return.
    pub limit: Option<u32>,
    /// The datasource's name or uid. Default: the default one of its kind,
    /// else the first.
    pub datasource: Option<String>,
}

/// Read one Grafana path through the run proxy, with query parameters.
/// Prints Grafana's body as it is.
pub async fn get(path: &str, params: &[(String, String)]) -> Result<String, String> {
    let g = crate::linear_cli::grant()?;
    let client = reqwest::Client::builder()
        // The server allows Grafana 300s; leave room for the hop.
        .timeout(std::time::Duration::from_secs(330))
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    let path = path.trim_start_matches('/');
    let resp = client
        .get(format!("{}/api/run/grafana/{path}", g.base))
        .bearer_auth(&g.token)
        .query(params)
        .send()
        .await
        .map_err(|e| format!("could not reach the harness at {}: {e}", g.base))?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("could not read the response: {e}"))?;
    if status.is_success() {
        return Ok(text);
    }
    Err(format!(
        "{status}: {}",
        crate::http::integrations::error_message(&text)
    ))
}

/// Run `query` against a Loki or Prometheus datasource. Prints the
/// datasource's JSON answer.
pub async fn query(kind: QueryKind, q: &Query) -> Result<String, String> {
    let uid = datasource_uid(kind, q.datasource.as_deref()).await?;
    let (range, params) = query_params(kind, q)?;
    get(
        &format!("api/datasources/proxy/uid/{uid}/{}", kind.api(range)),
        &params,
    )
    .await
}

/// The request parameters, and whether this is a range query.
pub(crate) fn query_params(
    kind: QueryKind,
    q: &Query,
) -> Result<(bool, Vec<(String, String)>), String> {
    let range = q.start.is_some() || q.end.is_some();
    if range && (q.start.is_none() || q.end.is_none()) {
        return Err("a range query needs both --start and --end".into());
    }
    if range && q.time.is_some() {
        return Err("--time is for an instant query; a range query takes --start and --end".into());
    }
    if !range && q.step.is_some() {
        return Err("--step is for a range query; give --start and --end too".into());
    }
    let mut params = vec![("query".to_string(), q.expr.clone())];
    let mut put = |k: &str, v: &Option<String>| {
        if let Some(v) = v {
            params.push((k.to_string(), v.clone()));
        }
    };
    put("time", &q.time);
    put("start", &q.start);
    put("end", &q.end);
    put("step", &q.step);
    if let Some(limit) = q.limit {
        if kind == QueryKind::Prometheus {
            return Err("--limit is for Loki queries".into());
        }
        params.push(("limit".into(), limit.to_string()));
    }
    Ok((range, params))
}

#[derive(Debug, Deserialize)]
pub(crate) struct Datasource {
    uid: String,
    name: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default, rename = "isDefault")]
    is_default: bool,
}

async fn datasource_uid(kind: QueryKind, wanted: Option<&str>) -> Result<String, String> {
    let list = get("api/datasources", &[]).await?;
    let all: Vec<Datasource> = serde_json::from_str(&list)
        .map_err(|e| format!("unexpected datasource list from Grafana: {e}"))?;
    pick_datasource(&all, kind, wanted)
}

pub(crate) fn pick_datasource(
    all: &[Datasource],
    kind: QueryKind,
    wanted: Option<&str>,
) -> Result<String, String> {
    let of_kind: Vec<&Datasource> = all
        .iter()
        .filter(|d| d.kind == kind.datasource_type())
        .collect();
    let names = || {
        of_kind
            .iter()
            .map(|d| d.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    if let Some(w) = wanted {
        return of_kind
            .iter()
            .find(|d| d.uid == w || d.name == w)
            .map(|d| d.uid.clone())
            .ok_or_else(|| {
                format!(
                    "no {} datasource named `{w}`; there is: {}",
                    kind.datasource_type(),
                    names()
                )
            });
    }
    of_kind
        .iter()
        .find(|d| d.is_default)
        .or(of_kind.first())
        .map(|d| d.uid.clone())
        .ok_or_else(|| format!("Grafana has no {} datasource", kind.datasource_type()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ds(uid: &str, name: &str, kind: &str, is_default: bool) -> Datasource {
        Datasource {
            uid: uid.into(),
            name: name.into(),
            kind: kind.into(),
            is_default,
        }
    }

    #[test]
    fn the_datasource_is_the_named_one_or_the_default_of_its_kind() {
        let all = [
            ds("p1", "Prometheus", "prometheus", true),
            ds("l1", "Loki old", "loki", false),
            ds("l2", "Loki", "loki", true),
        ];
        assert_eq!(pick_datasource(&all, QueryKind::Loki, None).unwrap(), "l2");
        assert_eq!(
            pick_datasource(&all, QueryKind::Loki, Some("Loki old")).unwrap(),
            "l1"
        );
        assert_eq!(
            pick_datasource(&all, QueryKind::Loki, Some("l1")).unwrap(),
            "l1"
        );
        let err = pick_datasource(&all, QueryKind::Loki, Some("Prometheus")).unwrap_err();
        assert!(err.contains("Loki old, Loki"), "{err}");
        assert!(pick_datasource(&all[..1], QueryKind::Loki, None).is_err());
    }

    #[test]
    fn instant_and_range_queries_take_their_own_parameters() {
        let instant = Query {
            expr: "sum(rate(x[5m]))".into(),
            time: Some("1795772127".into()),
            ..Query::default()
        };
        let (range, params) = query_params(QueryKind::Prometheus, &instant).unwrap();
        assert!(!range);
        assert_eq!(params[1], ("time".into(), "1795772127".into()));

        let ranged = Query {
            expr: "{job=\"web-vitals\"}".into(),
            start: Some("1".into()),
            end: Some("2".into()),
            step: Some("1h".into()),
            limit: Some(10),
            ..Query::default()
        };
        let (range, params) = query_params(QueryKind::Loki, &ranged).unwrap();
        assert!(range);
        assert_eq!(params.len(), 5);
        assert_eq!(QueryKind::Loki.api(range), "loki/api/v1/query_range");

        let half = Query {
            start: Some("1".into()),
            ..Query::default()
        };
        assert!(query_params(QueryKind::Loki, &half).is_err());
        let step_only = Query {
            step: Some("1h".into()),
            ..Query::default()
        };
        assert!(query_params(QueryKind::Loki, &step_only).is_err());
        let prom_limit = Query {
            limit: Some(5),
            ..Query::default()
        };
        assert!(query_params(QueryKind::Prometheus, &prom_limit).is_err());
    }
}
