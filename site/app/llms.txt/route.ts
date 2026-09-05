import { siteConfig } from "@/lib/config"
import { docsSections } from "@/lib/docs-order"

export const dynamic = "force-static"

/**
 * https://llmstxt.org — a curated index for language models, so an assistant
 * reading this project does not have to infer its shape from rendered HTML and
 * navigation chrome.
 *
 * This file is the map; `/llms-full.txt` is the territory.
 */
export function GET() {
  const body = docsSections()
    .map((section) => {
      const links = section.pages
        .map((page) => {
          const url = new URL(page.url, siteConfig.url).toString()
          const description = page.data.description
          return `- [${page.data.title}](${url})${description ? `: ${description}` : ""}`
        })
        .join("\n")
      return `## ${section.title}\n\n${links}`
    })
    .join("\n\n")

  const why = new URL("/why", siteConfig.url).toString()

  const text = `# ${siteConfig.name}

> ${siteConfig.description}

${siteConfig.name} builds prompts and manages lifecycle; the coding agents it
drives decide how to execute. A task becomes a run of a user-authored workflow
DAG, executed in an isolated git worktree, and ends in a pull request.

The full documentation as a single file is at ${new URL("/llms-full.txt", siteConfig.url).toString()}.

## Start here

- [Why a pipeline, not a loop](${why}): where this sits next to CI/CD, and an honest answer to why you should not just run a coding agent in a loop.

${body}

## Source

- [Repository](${siteConfig.links.github}): source, issues and releases.
`

  return new Response(text, {
    headers: { "Content-Type": "text/plain; charset=utf-8" },
  })
}
