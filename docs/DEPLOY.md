# Deploying ai-harness to `home-ops` — handoff brief

This is a self-contained brief for the **home-ops cluster AI**. It describes how
to add `ai-harness` (the control plane) to the Flux GitOps repo. It mirrors the
existing **`archon`** app (`kubernetes/apps/automation/archon/`), which is the
closest analog (custom ghcr image, CloudNativePG, Envoy route, SOPS secret).

> Scope note: this deploys the **control plane only**. Workflow runs currently
> execute **in-process inside this pod** (no per-run Job pods yet — that's a later
> phase), so size the pod for agent subprocesses. **Provider credentials are NOT
> in this secret** — they're entered in the harness UI and stored encrypted in
> Postgres (UI credentials feature). So the cluster secret is infra-only.

## 1. The image

- **`ghcr.io/marknygaard/ai-harness:latest`** (also `:main-<sha>`), built and
  pushed by the ai-harness repo's `Image` GitHub Action on every merge to `main`.
  `linux/amd64`. It bundles the web UI (embedded in the binary) plus the agent
  CLIs (`claude`, `codex`, `omp`), `git`, and `mise`.
- Entrypoint runs `harness serve`. It already sets `HARNESS_HTTP_ADDR=0.0.0.0:8080`
  and listens on **8080**; health is **`GET /health`**.

## 2. What it needs

| Need | How |
|---|---|
| Postgres | A database + role on the existing CNPG `postgres` cluster (`database` ns) |
| Config | env `HARNESS_DATABASE_URL` (required), `HARNESS_API_TOKEN` (optional, gates the API) |
| Persistence | a PVC mounted at **`/home/harness`** — holds run artifacts + (later) UI-entered agent credentials, so they survive restarts |
| Ingress | an Envoy `HTTPRoute` on `envoy-internal` at `harness.${SECRET_DOMAIN}` |

## 3. Files to create — `kubernetes/apps/automation/ai-harness/`

Mirror `archon`. Adapt these to current conventions/versions.

### `ks.yaml`
```yaml
---
apiVersion: kustomize.toolkit.fluxcd.io/v1
kind: Kustomization
metadata:
  name: ai-harness
spec:
  dependsOn:
    - name: cloudnativepg-cluster
      namespace: database
  interval: 1h
  path: ./kubernetes/apps/automation/ai-harness/app
  postBuild:
    substituteFrom:
      - name: cluster-secrets
        kind: Secret
  prune: true
  sourceRef:
    kind: GitRepository
    name: flux-system
    namespace: flux-system
  targetNamespace: automation
  wait: false
```
Then add `./ai-harness/ks.yaml` to `kubernetes/apps/automation/kustomization.yaml`.

### `app/ocirepository.yaml` — the bjw-s app-template chart (copy archon's, name `ai-harness`).

### `app/helmrelease.yaml`
```yaml
---
apiVersion: helm.toolkit.fluxcd.io/v2
kind: HelmRelease
metadata:
  name: ai-harness
spec:
  chartRef:
    kind: OCIRepository
    name: ai-harness
  interval: 1h
  values:
    controllers:
      ai-harness:
        annotations:
          reloader.stakater.com/auto: "true"
        containers:
          app:
            image:
              repository: ghcr.io/marknygaard/ai-harness
              tag: latest
            env:
              TZ: Europe/Copenhagen
              HARNESS_HTTP_ADDR: 0.0.0.0:8080
              HARNESS_PROJECT_ROOT: /home/harness/project
            envFrom:
              - secretRef:
                  name: ai-harness-secret
            probes:
              liveness: { enabled: true, custom: true, spec: { httpGet: { path: /health, port: 8080 }, periodSeconds: 30 } }
              readiness: { enabled: true, custom: true, spec: { httpGet: { path: /health, port: 8080 }, periodSeconds: 10 } }
              startup: { enabled: true, custom: true, spec: { httpGet: { path: /health, port: 8080 }, failureThreshold: 60, periodSeconds: 5 } }
            resources:
              requests: { cpu: 100m, memory: 512Mi }
              limits: { memory: 4Gi }
            securityContext:
              allowPrivilegeEscalation: false
              readOnlyRootFilesystem: false
              capabilities: { drop: ["ALL"] }
    defaultPodOptions:
      securityContext:
        runAsNonRoot: true
        runAsUser: 1000
        runAsGroup: 1000
        fsGroup: 1000
        fsGroupChangePolicy: OnRootMismatch
    service:
      app:
        ports:
          http:
            port: 8080
    persistence:
      home:
        existingClaim: ai-harness
        globalMounts:
          - path: /home/harness
    route:
      app:
        hostnames: ["harness.${SECRET_DOMAIN}"]
        parentRefs:
          - name: envoy-internal
            namespace: network
            sectionName: https
```

### `app/pvc.yaml` — `local-path`, ~5Gi, name `ai-harness`.

### `app/secret.sops.yaml` (SOPS-encrypt with the cluster age key)
```yaml
apiVersion: v1
kind: Secret
metadata:
  name: ai-harness-secret
stringData:
  # CNPG role/db created below. Use the CNPG-generated password.
  HARNESS_DATABASE_URL: postgresql://ai-harness:<password>@postgres-rw.database.svc.cluster.local:5432/ai-harness
  # 32 random bytes, base64. Encrypts UI-entered provider credentials at rest.
  # Generate: `openssl rand -base64 32`. Keep it stable — rotating it makes
  # previously stored credentials undecryptable (just re-enter them in the UI).
  HARNESS_SECRET_KEY: <base64-of-32-random-bytes>
  # Optional: gate the API/UI behind a bearer token.
  HARNESS_API_TOKEN: <random-token>
```
**No provider tokens here** — Claude/Codex/Kimi credentials are entered in the
harness **UI** (encrypted at rest in Postgres under `HARNESS_SECRET_KEY`). The
only secrets in SOPS are the DB URL, this encryption key, and an optional API
token — all infra, never provider tokens.

### `app/kustomization.yaml` — list the resources above (copy archon's).

## 4. Postgres — database + role on the existing CNPG cluster

The `postgres` cluster bootstraps only one database, so create ours. If your CNPG
version supports declarative CRs (v1.22+), add to the cluster app:
```yaml
---
apiVersion: postgresql.cnpg.io/v1
kind: Database
metadata:
  name: ai-harness
spec:
  cluster: { name: postgres }
  name: ai-harness
  owner: ai-harness
---
apiVersion: postgresql.cnpg.io/v1
kind: Role  # or manage the role/secret via your existing pattern
metadata:
  name: ai-harness
spec:
  cluster: { name: postgres }
  name: ai-harness
  ensure: present
  login: true
```
Otherwise create the role+db with a one-off `psql` against `postgres-rw` and put
the password in the secret above. (Match whatever pattern `archon`'s `archon` db
uses today.)

## 5. Verify

- `flux reconcile kustomization ai-harness -n flux-system`
- Pod healthy on `/health`; open `https://harness.${SECRET_DOMAIN}` → the Runs UI.
- Submit a run from the UI (or `POST /api/runs`). **Echo runs + the editor work
  immediately.** **Real agent runs** (`real: true`) need the UI credentials
  feature (next ai-harness PR) to supply provider auth — until then the agent CLIs
  are present but unauthenticated.

## 6. Not yet (later ai-harness phases)
- UI-managed provider credentials (so Claude/Codex/Kimi auth is entered in the UI).
- Per-run Kubernetes **Job** executor + toolchain provisioning (runs are in-process today).
- Linear/cron triggers.
