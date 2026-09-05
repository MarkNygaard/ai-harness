import type { Metadata } from "next"
import Link from "next/link"

import { siteConfig } from "@/lib/config"
import { SiteFooter } from "@/components/site-footer"

const tagline = "Ship a feature without writing it"

export const metadata: Metadata = {
  title: { absolute: `${siteConfig.name}. ${tagline}.` },
  description: siteConfig.description,
  alternates: { canonical: "/" },
}

const features = [
  {
    title: "Workflow DAGs",
    body: "Write pipelines in YAML or draw them in the editor. Explore, plan, implement, validate, open the PR, review. Nodes gate on conditions and pass output to each other, and a loop can run until something is true.",
    href: "/docs/workflows/authoring",
  },
  {
    title: "Many agents, one pipeline",
    body: "Claude Code, Codex, Pi and Cursor nodes in the same run, each on its own model. The harness writes the prompts and runs the pipeline. The agents decide how to do the work.",
    href: "/docs/workflows/authoring",
  },
  {
    title: "Three ways to trigger",
    body: "The dashboard, an MCP endpoint so an assistant you are already talking to can start a run, or Linear. Delegate an issue and it moves through your columns on its own.",
    href: "/docs/triggers/linear-connect",
  },
  {
    title: "Isolated by default",
    body: "Every run works in its own git worktree off the base branch, thrown away afterwards. Credentials are encrypted at rest, and a run cannot read the database URL out of its own environment.",
    href: "/docs/operating/deploy",
  },
  {
    title: "Toolchains on demand",
    body: "Declare what a project needs and mise installs it on the first run, then caches it. Adding a language does not mean rebuilding the image.",
    href: "/docs/operating/deploy",
  },
  {
    title: "One binary, one Postgres",
    body: `One ${siteConfig.binary} binary with the dashboard inside it, and a Postgres database. Runs are child processes of that binary, so the same image works under Kubernetes or plain Docker.`,
    href: "/docs/operating/deploy",
  },
]

const loop = [
  {
    title: "Write the issue",
    body: "Describe what should be true when it is done, and how you will test it. Those are the acceptance criteria and the instructions at the same time.",
    who: "Whoever wants the change",
  },
  {
    title: "Hand it over",
    body: "Delegate the issue. It plans, writes the code, then reviews that code with models that did not write it, and opens a pull request.",
    who: "The harness",
  },
  {
    title: "Test it",
    body: "Against the criteria you wrote. Trust your test over the run's own summary. It is usually right, and usually is the dangerous part.",
    who: "Whoever wrote the issue",
  },
  {
    title: "Review the code",
    body: "By now the obvious problems are gone, which is not the same as the code being right. You can automate this step away. We think you should not.",
    who: "A developer",
  },
  {
    title: "Write the rule",
    body: "Made the same review comment twice? That is a missing rule. Put it in the project's CLAUDE.md and later runs get it right the first time.",
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
          Write the issue. Hand it to {siteConfig.name}. It plans, writes the
          code, reviews that code with a model that did not write it, and opens
          a pull request. You test it against what you asked for. Then a
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
          The person who wrote the acceptance criteria can tell whether it
          does the right thing. The developer can tell whether it does it the
          right way. Those are different questions, and splitting them is what
          makes it safe to let someone ship who cannot read a diff.
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
          Already running an agent in a loop?{" "}
          <Link
            href="/why"
            className="underline underline-offset-4 hover:text-foreground"
          >
            Why a pipeline, not a loop
          </Link>{" "}
          is the honest comparison, including when a loop is the better tool.
        </p>

        <p className="mt-3 max-w-2xl text-sm leading-relaxed text-muted-foreground">
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
          covers the developer&apos;s side. It explains the file every run
          reads, and why review gets lighter over time instead of staying
          flat.
        </p>
      </section>

      <section className="mx-auto w-full max-w-6xl px-6 pb-16">
        <h2 className="text-2xl font-semibold tracking-tight">
          Someone has to run it once
        </h2>
        <p className="mt-3 max-w-2xl leading-relaxed text-muted-foreground">
          One container and a Postgres database, on your own hardware. After
          that, nobody needs a terminal to use it.
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
          covers Kubernetes, what has to survive a restart, and how big the
          volume needs to be.
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
          Nine ship ready to run. Copy one into a project&apos;s{" "}
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
