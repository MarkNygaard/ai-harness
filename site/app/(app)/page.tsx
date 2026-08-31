import type { Metadata } from "next"
import Link from "next/link"

import { siteConfig } from "@/lib/config"
import { SiteFooter } from "@/components/site-footer"

const tagline = "Ship a feature without writing it"

export const metadata: Metadata = {
  title: { absolute: `${siteConfig.name} — ${tagline}` },
  description: siteConfig.description,
  alternates: { canonical: "/" },
}

const features = [
  {
    title: "Workflow DAGs",
    body: "Author multi-node pipelines — explore, plan, implement, validate, PR, review — in YAML or the visual editor. Nodes gate on conditions, wire one node's output into the next, and support loop/until constructs.",
    href: "/docs/workflows/authoring",
  },
  {
    title: "Many agents, one pipeline",
    body: "Claude Code, Codex, Pi/Kimi and Cursor nodes in the same run, each picking its own model. The harness builds the prompts and manages lifecycle; the agents decide how to execute.",
    href: "/docs/workflows/authoring",
  },
  {
    title: "Three ways to trigger",
    body: "The web UI, an MCP-over-HTTP endpoint so a connected assistant can author and fire workflows, or Linear delegation — assign an issue and it walks the status map on its own.",
    href: "/docs/triggers/linear-connect",
  },
  {
    title: "Isolated by default",
    body: "Every run operates on a git worktree off the project's base branch. Credentials are encrypted at rest, and control-plane secrets are scrubbed from the agent processes a run spawns.",
    href: "/docs/operating/deploy",
  },
  {
    title: "Toolchains on demand",
    body: "Declare a project's toolchains and mise installs them at run time, cached on the data volume. No image rebuild to add a language.",
    href: "/docs/operating/deploy",
  },
  {
    title: "One binary, one Postgres",
    body: `The control plane is a single ${siteConfig.binary} binary with the dashboard bundled in, backed by Postgres. Runs execute as local child processes, so the same image works under Kubernetes or plain Docker.`,
    href: "/docs/operating/deploy",
  },
]

const loop = [
  {
    title: "Write the issue",
    body: "Describe what should be true when it is done, and how you will test it. That doubles as the acceptance criteria and as the instructions.",
    who: "Whoever wants the change",
  },
  {
    title: "Hand it over",
    body: "Delegate the issue. It plans, implements, and runs review passes with different models than the one that wrote the code, then opens a pull request.",
    who: "The harness",
  },
  {
    title: "Test it",
    body: "Against the criteria you wrote. Trust your test over its summary — the run is usually right, and usually is exactly the failure mode to guard against.",
    who: "Whoever wrote the issue",
  },
  {
    title: "Review the code",
    body: "A human approval is still required to merge. The obvious problems are generally gone by now, which is not the same as it being right.",
    who: "A developer",
  },
  {
    title: "Write the rule",
    body: "Made the same review comment twice? That is a missing rule. Move it into the project's CLAUDE.md and every later run gets it right the first time.",
    who: "A developer",
  },
]

const workflows = [
  ["idea-to-pr", "a task becomes a reviewed PR"],
  ["revise-pr", "address review feedback on an open PR"],
  ["merge-pr", "resolve conflicts and merge a ready PR"],
  ["architect", "behaviour-preserving codebase health sweep"],
  ["geo-audit", "audit a site for AI-search readiness"],
  ["judge-ab", "score an A/B model comparison"],
]

/**
 * WebSite plus SoftwareApplication: the first tells a crawler what this site
 * *is* and anchors the docs' `isPartOf`; the second is what a search engine or
 * assistant reads to answer "what is this, what does it run on, what does it
 * cost" without inferring it from marketing prose.
 */
const jsonLd = {
  "@context": "https://schema.org",
  "@graph": [
    {
      "@type": "WebSite",
      "@id": `${siteConfig.url}/#website`,
      url: siteConfig.url,
      name: siteConfig.name,
      description: siteConfig.description,
      inLanguage: "en",
    },
    {
      "@type": "SoftwareApplication",
      "@id": `${siteConfig.url}/#software`,
      name: siteConfig.name,
      description: siteConfig.description,
      applicationCategory: "DeveloperApplication",
      operatingSystem: "Linux, Docker, Kubernetes",
      url: siteConfig.url,
      codeRepository: siteConfig.links.github,
      license: "https://opensource.org/licenses/MIT",
      isAccessibleForFree: true,
      offers: { "@type": "Offer", price: "0", priceCurrency: "USD" },
    },
  ],
}

export default function HomePage() {
  return (
    <>
      <script
        type="application/ld+json"
        // Trusted, locally constructed values -- no user input reaches this.
        dangerouslySetInnerHTML={{ __html: JSON.stringify(jsonLd) }}
      />
      <section className="mx-auto w-full max-w-6xl px-6 pt-20 pb-16">
        <p className="text-sm font-medium text-accent-orange">
          Self-hosted · open source · MIT
        </p>
        <h1 className="mt-4 max-w-3xl text-4xl font-semibold tracking-tight text-balance sm:text-5xl">
          {tagline}
        </h1>
        <p className="mt-5 max-w-2xl text-lg leading-relaxed text-muted-foreground">
          Write the issue. Hand it to {siteConfig.name}. It plans, implements,
          reviews its own work with a different model than wrote it, and opens a
          pull request. You test that against what you asked for — then a
          developer reviews the code.
        </p>

        <div className="mt-8 flex flex-wrap items-center gap-3">
          <Link
            href="/docs"
            className="inline-flex h-9 items-center rounded-md bg-primary px-4 text-sm font-medium text-primary-foreground transition-opacity hover:opacity-90"
          >
            Get started
          </Link>
          <a
            href={siteConfig.links.github}
            target="_blank"
            rel="noreferrer"
            className="inline-flex h-9 items-center rounded-md border border-border px-4 text-sm font-medium transition-colors hover:bg-accent"
          >
            View source
          </a>
        </div>

      </section>

      <section className="mx-auto w-full max-w-6xl px-6 pb-16">
        <h2 className="text-2xl font-semibold tracking-tight">
          Two people check two different things
        </h2>
        <p className="mt-3 max-w-2xl leading-relaxed text-muted-foreground">
          The person who wrote the acceptance criteria is the person who can
          tell whether it does the right thing. The developer is the person who
          can tell whether it does it the right way. Neither is checking what
          they are worse at — which is what makes it safe to let someone ship
          who cannot read the diff.
        </p>

        <ol className="mt-8 grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
          {loop.map((step, i) => (
            <li
              key={step.title}
              className="rounded-lg border border-border bg-card p-5"
            >
              <span className="font-mono text-xs text-muted-foreground">
                {String(i + 1).padStart(2, "0")}
              </span>
              <h3 className="mt-2 font-medium">{step.title}</h3>
              <p className="mt-1.5 text-sm leading-relaxed text-muted-foreground">
                {step.body}
              </p>
              <p className="mt-3 text-xs font-medium text-accent-orange">
                {step.who}
              </p>
            </li>
          ))}
        </ol>

        <p className="mt-6 max-w-2xl text-sm leading-relaxed text-muted-foreground">
          <Link
            href="/docs/shipping-without-code"
            className="underline underline-offset-4 hover:text-foreground"
          >
            Shipping without writing code
          </Link>{" "}
          walks through the loop from the requesting side, and{" "}
          <Link
            href="/docs/project-rules"
            className="underline underline-offset-4 hover:text-foreground"
          >
            project rules
          </Link>{" "}
          covers the developer&apos;s side — the file every run reads, and the
          reason review effort goes down over time instead of staying flat.
        </p>
      </section>

      <section className="mx-auto w-full max-w-6xl px-6 pb-16">
        <h2 className="text-2xl font-semibold tracking-tight">
          Someone has to run it once
        </h2>
        <p className="mt-3 max-w-2xl leading-relaxed text-muted-foreground">
          One container and a Postgres database, on your own infrastructure.
          After that the loop above needs no terminal.
        </p>
        <pre className="mt-6 max-w-3xl overflow-x-auto rounded-lg border border-border bg-card p-4 text-sm whitespace-pre">
          <code>{`docker run -p 8080:8080 \
  -e ${siteConfig.envPrefix}DATABASE_URL=postgres://... \
  -e ${siteConfig.envPrefix}SECRET_KEY=... \
  ${siteConfig.image}`}</code>
        </pre>
        <p className="mt-4 text-sm text-muted-foreground">
          <Link
            href="/docs/operating/deploy"
            className="underline underline-offset-4 hover:text-foreground"
          >
            Deploying
          </Link>{" "}
          covers Kubernetes, what has to persist, and sizing the volume.
        </p>
      </section>

      <section className="mx-auto w-full max-w-6xl px-6 pb-16">
        <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
          {features.map((feature) => (
            <Link
              key={feature.title}
              href={feature.href}
              className="rounded-lg border border-border bg-card p-5 transition-colors hover:border-ring"
            >
              <h2 className="font-medium">{feature.title}</h2>
              <p className="mt-2 text-sm leading-relaxed text-muted-foreground">
                {feature.body}
              </p>
            </Link>
          ))}
        </div>
      </section>

      <section className="mx-auto w-full max-w-6xl px-6 pb-24">
        <h2 className="text-2xl font-semibold tracking-tight">
          Workflows in the box
        </h2>
        <p className="mt-2 max-w-2xl text-muted-foreground">
          Several ship ready to run, or to fork into a project&apos;s{" "}
          <code className="rounded bg-muted px-1.5 py-0.5 text-sm">
            {siteConfig.configDir}/workflows/
          </code>
          .
        </p>
        <dl className="mt-6 divide-y divide-border border-y border-border">
          {workflows.map(([name, what]) => (
            <div key={name} className="flex flex-wrap gap-x-4 gap-y-1 py-3">
              <dt className="w-40 font-mono text-sm">{name}</dt>
              <dd className="text-sm text-muted-foreground">{what}</dd>
            </div>
          ))}
        </dl>
      </section>

      <SiteFooter />
    </>
  )
}
