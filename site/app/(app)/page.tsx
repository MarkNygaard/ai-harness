import type { Metadata } from "next"
import Link from "next/link"

import { siteConfig } from "@/lib/config"
import { SiteFooter } from "@/components/site-footer"

const tagline = "From a task to a reviewed pull request"

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

const workflows = [
  ["idea-to-pr", "a task becomes a reviewed PR"],
  ["revise-pr", "address review feedback on an open PR"],
  ["merge-pr", "resolve conflicts and merge a ready PR"],
  ["architect", "behaviour-preserving codebase health sweep"],
  ["geo-audit", "audit a site for AI-search readiness"],
  ["judge-ab", "score an A/B model comparison"],
]

export default function HomePage() {
  return (
    <>
      <section className="mx-auto w-full max-w-6xl px-6 pt-20 pb-16">
        <p className="text-sm font-medium text-accent-orange">
          Rust-native · self-hosted · MIT
        </p>
        <h1 className="mt-4 max-w-3xl text-4xl font-semibold tracking-tight text-balance sm:text-5xl">
          {tagline}
        </h1>
        <p className="mt-5 max-w-2xl text-lg leading-relaxed text-muted-foreground">
          {siteConfig.name} is an orchestration layer for AI coding agents. It
          turns a task — typed in a UI, sent over MCP, or pulled from a Linear
          column — into a run of a workflow DAG you authored, drives coding
          agents through it in an isolated git worktree, and opens a pull
          request at the end.
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

        <pre className="mt-10 max-w-3xl overflow-x-auto rounded-lg border border-border bg-card p-4 text-sm whitespace-pre">
          <code>{`docker run -p 9800:9800 \\
  -e ${siteConfig.envPrefix}DATABASE_URL=postgres://... \\
  -e ${siteConfig.envPrefix}SECRET_KEY=... \\
  ${siteConfig.image}`}</code>
        </pre>
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
