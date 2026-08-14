# Connecting Linear

The harness writes to Linear on its own: it moves claimed issues through the
status map, comments when a run starts/finishes, attaches the run link, and
applies the failed label when it gives up.

**Who Linear records as the author of those writes depends on the credential.**

| Credential | Attribution |
|---|---|
| **OAuth install with `actor=app`** (recommended) | The application. Comments read as posted by the app. |
| Personal API key (legacy) | The human who minted the key. Every bot comment looks like they wrote it. |

Connect the workspace once and the attribution problem goes away.

**One workspace, one install.** The Linear credential is global — it lives on the
**Credentials page** under *Integrations*, not per project. The identity being
connected is the *app*, which is the same for every project; only the trigger
*bindings* (team, status map, eligibility label) are per project, and those stay
on the Projects page.

## How work reaches the harness

Two gates, and **both** must hold before anything starts:

1. **Delegated to the harness.** Assigning an issue to the app sets it as the
   issue's *delegate* (not its assignee — a human keeps ownership). This replaced
   the old "AI Eligible" label: a person choosing to delegate is a better signal
   than a label anyone can add. @-mentioning the app counts too.
2. **In the binding's source status.** The trigger you configured still decides
   *when* work is ready. Delegating an issue that's sitting in Backlog does not
   start it.

Delegation says *who* should do the work; the status says *when*.

Two paths apply those same gates:

- **Webhook (fast).** Delegating opens an **agent session** — a thread on the
  issue where the harness reports progress — and Linear posts it to the harness
  immediately. If the issue is in the source status, the run starts in seconds. If
  it isn't, the harness says so in the session instead of starting.
- **Poller (reconciliation).** Every tick, each `enabled` + `live` binding looks
  for issues that are delegated to the harness *and* in its source status. This is
  the safety net: if the harness was down or a webhook delivery failed, the
  delegated issue is still picked up on a later tick. It is also what drives
  stage-to-stage pipelines (e.g. `merge-pr` watching *Ready for merge*).

Because the poller now checks delegation, leaving `live` on is safe — it can no
longer claim an issue nobody handed it. If the harness doesn't know its own app
user id (an install predating that being recorded), the poller claims **nothing**
rather than everything, and logs why.

## One-time: create a Linear OAuth application

In Linear: **Settings → API → OAuth applications → Create new**.

- **Callback URL** — the exact URL shown in the harness (Credentials →
  Integrations → Linear). It is
  `${HARNESS_PUBLIC_URL}/api/linear/oauth/callback`, so
  `HARNESS_PUBLIC_URL` (or `server.public_url`) **must** be set; Linear requires
  an exact match and the harness refuses to start the flow without it.
- **Webhook** — enable it, point it at the **webhook URL** the same panel shows
  (`${HARNESS_PUBLIC_URL}/api/linear/webhook`), and subscribe it to **Agent
  session events**. Copy its **signing secret**: every inbound delegation is
  verified against it, and without it the webhook rejects everything.
- Keep the app private to your workspace unless you intend to distribute it.
- Give it the name and avatar you want to see on the comments and activities it
  writes — that's the identity your team will see in the issue thread.

Copy the **client ID**, **client secret** and **webhook signing secret**.

## Connect

1. **Credentials → Integrations → Linear → Connect.**
2. Paste the **OAuth client ID**, **client secret** and **webhook signing
   secret**, saving each. They are encrypted at rest (AES-256-GCM) like every
   other credential.
3. Click **Connect as app**. Linear asks a workspace admin to authorize —
   `actor=app` installs require admin permission — and redirects back to the
   Credentials page with the result.
4. The panel then reads **app install**, names the workspace, and shows two
   readiness lines: whether the token carries the agent scopes, and whether the
   webhook secret is stored. Both must be satisfied for delegation to work.

Requested scopes: `read` (discovery and issue preview), `write` (status moves,
comments, labels, attachments), `app:assignable` (can be delegated an issue) and
`app:mentionable` (can be @-mentioned). Not `admin`.

**If you connected before delegation was added**, your stored token predates the
agent scopes. The poller keeps working, but delegation won't — the panel says so.
Click **Reconnect**.

## Per project: bind a team

Delegation carries no configuration of its own, so the issue's **team** is matched
against the Linear trigger bindings (Projects → the project → the ⚡ icon). The
matching binding supplies the project, the workflow to run, the base branch, the
source status that gates it, and the rest of the status map.

**The binding must be `enabled`.** Unchecking it stops work arriving by *either*
route — delegation as well as the poller. `live` is poller-only (claim vs. dry-run),
so a delegation-only setup is `enabled` on, `live` off. This matters when two
bindings share a source status: disabling the one you don't want is how you choose
between them.

With exactly one enabled binding overall, it is used regardless of team (the status
gate still applies). With none that match, the harness replies in the session saying
so rather than guessing.

## Images pasted into an issue

Screenshots and images in an issue's description or comments are **downloaded and
handed to the agent as files**, so a ticket saying "fix this, see screenshot"
actually works.

Linear's upload URLs are private — an unauthenticated request gets a 401, and the
agent holds no Linear credential — so forwarding the link would be useless. Instead
the harness fetches each image with the workspace credential, writes it next to the
project checkouts (**outside** every worktree, so a screenshot can never be
committed into a PR), and rewrites the link in the task text to that path. Agents
read images from a path natively, so this needs nothing from the agent side.

Worth knowing:

- **Nothing is resized or re-encoded.** The agent's own tooling downscales on the
  way to the model. Sizes vary enormously — a UI screenshot is a few hundred KB, a
  photograph tens of MB — and both work.
- **Very large screenshots lose fine text.** Downscaling a 5K capture halves small
  UI labels; crop to the relevant region if the detail matters.
- **Only `png`, `jpeg`, `gif` and `webp`** are passed on. No SVG (it can carry
  script), and nothing non-image.
- **Only `uploads.linear.app` is ever fetched.** Issue text is written by anyone who
  can file an issue, so treating arbitrary URLs in it as fetchable would be an SSRF
  hole. Links to other hosts are left as text, untouched.
- **At most 5 images per task**, and any single file over 25MB is skipped.
- **Failure is never fatal.** If a download fails the original URL stays in the text
  and the run proceeds — an image is a bonus, not a prerequisite.
- On a **text-only** model the agent simply won't see the image; omp substitutes
  `[image omitted: model does not support vision]` rather than erroring.

Files live under `<projects-dir>/../attachments/<issue>/`, overridable with
`HARNESS_ATTACHMENTS_DIR`, and are **swept hourly**: a task's directory is deleted
once nothing has written to it for a week (`HARNESS_ATTACHMENTS_TTL_HOURS`). The
sweep also runs on startup, so anything a crash left behind is cleared.

The lifetime is deliberately age-based rather than tied to a run finishing. A run
reads its images at any point, retries re-read them, and a rerun a week later simply
re-downloads — so wall-clock age needs no coordination with run state and can't
delete files a live run is about to open. A week is far longer than any run, and
re-downloading is cheap.

## What the harness reports back

Inside the agent session, as Linear agent activities rather than plain comments:

| When | Activity |
|---|---|
| Immediately on delegation | `thought` — "Picking up COR-12…" (Linear marks a session unresponsive without one inside **10 seconds**, so this is emitted before any slower work) |
| As each workflow step finishes | `action` — "Finished create-plan" (failures and cancellations are reported too) |
| When the **poller** claims an issue | it opens a session of its own first, so a poller-claimed run reports into a thread rather than posting detached comments |
| Every ~10 minutes of silence | `thought` — "Still working — `implement-tasks` has been running for 20 minutes" |
| Delegated, but not in the source status | `error` — names the status it's in and what to do |
| Delegated, but no binding covers the team | `error` — asks for a binding |
| Run started | `action` — the workflow name, with a link to the run |
| PR opened | `action` — moved to In Review |
| Run completed | `response` — which also marks the session complete |
| Run failed, budget spent | `error` |
| Run failed, retrying | `thought` (not `error` — that would close the session before the retry) |

A poller-claimed run has no session, so it still gets the plain issue comments it
always did.

Follow-up messages in a session are acknowledged honestly: the harness cannot
change course mid-run yet, so it says so rather than silently ignoring you.

## Tokens

The access token lasts ~24 hours; the harness refreshes it from the stored
refresh token about 5 minutes before expiry, serialized so two concurrent
refreshes can't spend the same single-use refresh token. Linear invalidates the
old pair on each refresh, so the new one is persisted immediately.

If a refresh is rejected (typically `invalid_grant` — the refresh token was
revoked or already spent), the failure is recorded on the credential and the
panel switches to **reconnect needed**. Click **Connect as app** again.

**Disconnect** revokes the token at Linear and clears it, keeping the client ID
and secret so reconnecting is one click. Use the credential's **Clear** button to
remove those too.

## Migrating from a personal API key

A **global** personal `api_key` keeps working untouched: the panel reports
**personal key**. To switch, connect the workspace as above — the OAuth token
takes precedence as soon as it exists — then clear the `api_key` field once
you've confirmed a run comments as the app.

A **per-project** Linear credential from an earlier version is **inert**: Linear
is now resolved from the global credential only, so a leftover project-scoped
`api_key` is neither read nor editable. If Linear was configured per project
before, connect the workspace once on the Credentials page. (Per-project *GitHub*
overrides are unaffected.)

Resolution, in full: the global `linear` credential's `access_token` if present,
else its `api_key`, else Linear is reported as not connected.

## Deploying behind Cloudflare Zero Trust (or any SSO proxy)

If the harness sits behind Cloudflare Access — or any proxy that demands an SSO
session — **the webhook will not work until you exempt its path.** Linear is a
server, not a browser: it has no Entra/Okta session and cannot get one, so Access
answers it with a login page instead of letting it reach the harness. Linear then
retries at 1 minute, 1 hour and 6 hours, and may disable the webhook.

Nothing in the harness can influence this: Access runs *in front of* the
application. The fix is one Cloudflare-side change.

**Add a second Access application scoped to the webhook path.** Access matches the
most specific application first, so a path-scoped app overrides the one covering
the hostname, and inherits nothing from it.

1. Zero Trust → **Access** → **Applications** → **Add an application** →
   **Self-hosted**. (Recent UI: *Access controls → Applications → Create new
   application → Self-hosted and private → Add public hostname*.)
2. Set the public hostname to your harness host **with the path**
   `api/linear/webhook`. It must match exactly — a wildcard does not cover its
   parent path, and this app must be *more* specific than your existing one.
3. Add one policy with **Action: Bypass** and **Include: Everyone**. That makes
   this path — and only this path — publicly reachable.
4. Leave your existing hostname-wide application untouched. The UI keeps
   requiring Entra.

What authenticates the endpoint after that is its **HMAC signature**, which is why
it is safe to expose: nothing is parsed before the signature is verified in
constant time, stale timestamps are rejected, and with no signing secret stored it
refuses everything. See below.

### Verify it before pointing Linear at it

From outside your network:

```sh
curl -i -X POST https://harness.example.com/api/linear/webhook \
  -H 'Content-Type: application/json' -d '{}'
```

Read the response to see **which gate answered**:

| Response | Meaning |
|---|---|
| HTML, or a 302 to your IdP | Cloudflare is still blocking — the bypass isn't matching. Check the path. |
| `412 {"error":"no Linear webhook signing secret configured"}` | ✅ Reached the harness. Save the signing secret on the Credentials page. |
| `401 {"error":"missing Linear-Signature header"}` | ✅ Reached the harness, secret stored, rejecting unsigned input correctly. |

A JSON error from the harness is success here — it means the request got through
and our own verification turned it away.

### If it still fails after the bypass

Access is not the only Cloudflare feature that can block a server-to-server POST:

- **Bot Fight Mode / Bot Management** — flags non-browser traffic. A common cause
  of webhooks failing behind Cloudflare. Check Security → Bots.
- **WAF custom or managed rules** issuing a Managed Challenge on that path. Add a
  skip rule for it if so.
- **"Require Access protection"** or an origin rule that rejects traffic without an
  Access JWT — that would undo the bypass.

A Cloudflare **Tunnel** is not a problem in itself: it routes the request, Access
is what blocks it.

## Security of the webhook endpoint

`POST /api/linear/webhook` is exempt from the API bearer-token middleware —
Linear cannot send our token — and is authenticated instead by the
`Linear-Signature` header: a hex HMAC-SHA256 of the **raw** request body under the
webhook signing secret, compared in constant time. A payload whose
`webhookTimestamp` is more than a minute from now is rejected as a replay. With no
secret stored the endpoint refuses everything rather than trusting unverified
input.

Once the signature checks out, every outcome returns 200 — including event types
we ignore — because Linear retries non-2xx responses and eventually disables a
webhook that keeps failing.

## Not yet

- **Mid-run steering.** A follow-up message in a session can't redirect a running
  workflow; it's acknowledged and you re-delegate after it finishes.
- **`elicitation` activities.** The harness never asks the session a question
  mid-run; it only reports.

## Filing an issue vs. starting one

"Create issue" (from a workflow's report UI) files the issue into the binding's
source status and deliberately **does not start it**. Filing and starting are
separate decisions: you capture the work when you find it, and delegate it to the
harness when you want it solved. Nothing picks it up in the meantime.

(Linear's `IssueCreateInput` does have a `delegateId`, so filing *and* delegating
in one step is possible if that ever becomes the preference.)

## Troubleshooting

| Symptom | Cause |
|---|---|
| Connect button disabled, "Save the OAuth client ID and secret first" | No `client_id`/`client_secret` stored. |
| "No public URL configured" | `HARNESS_PUBLIC_URL` / `server.public_url` unset. |
| `redirect_uri` mismatch error from Linear | The app's registered callback differs from the URL shown in the panel — they must match character for character. |
| "authorization expired or was already used" | The one-time `state` nonce was replayed or is older than 10 minutes (it is also lost if the server restarted mid-flow). Start again. |
| Comments still show a person's name | The panel reads **personal key** — a global `api_key` is still the credential in use. Connect as app, then clear the key. |
| The project's Linear trigger button disappeared | The bindings dialog only appears once Linear is connected. Connect on the Credentials page. |
| Delegating does nothing | Check the panel's two Delegation lines. Then check the OAuth app's webhook is enabled, subscribed to *Agent session events*, and pointed at the webhook URL shown. Server logs record every rejected delivery with the reason. |
| Session shows "unresponsive" in Linear | The acknowledging `thought` didn't land within 10s — usually the webhook never arrived, or the Linear credential can't write. The log line is `failed to acknowledge session`. |
| Session shows "stopped responding" mid-run | Linear marks a session stale after **30 minutes** without an activity. Progress activities and a ~10-minute heartbeat now keep it alive; if it still goes stale, check the poller is running (it is what posts them) and the logs for `failed to report into session`. Sending any later activity recovers the session. |
| "No enabled Linear trigger covers this issue’s team" | No **enabled** binding matches the issue’s team. Add one on the Projects page, or enable the existing one — a disabled binding is inert for delegation too. |
| "This issue is in X, and `wf` only starts from…" | Working as intended: the binding's source status is the gate. Move the issue there. |
| Delegated an issue in the right column and nothing happened | The poller logs `app user id is unknown` if the install predates that being recorded — reconnect the workspace. Otherwise check the webhook (see above). |
| Preview returns "app user id is unknown" | Same cause: reconnect. The preview mirrors the poller's gate, so it can't show delegated issues without knowing who the harness is. |
