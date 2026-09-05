import fs from "node:fs/promises"
import path from "node:path"

import { siteConfig } from "@/lib/config"
import { docsInOrder } from "@/lib/docs-order"

export const dynamic = "force-static"

const CONTENT_DIR = path.join(process.cwd(), "content/docs")
const PAGES_DIR = path.join(process.cwd(), "content/pages")

/** Strip YAML frontmatter — the title and description are re-emitted as prose. */
function stripFrontmatter(raw: string): string {
  if (!raw.startsWith("---")) return raw.trim()
  const end = raw.indexOf("\n---", 3)
  return end === -1 ? raw.trim() : raw.slice(end + 4).trim()
}

/**
 * Every documentation page concatenated as markdown, in reading order.
 *
 * Served from the source files rather than from the rendered pages: a model
 * asking how to author a workflow should get the markdown, not HTML with a
 * sidebar, a search box and a theme toggle in it.
 */
export async function GET() {
  const sections = await Promise.all(
    docsInOrder().map(async (page) => {
      const rel = (page as { path?: string }).path
      if (!rel) return null

      let raw: string
      try {
        raw = await fs.readFile(path.join(CONTENT_DIR, rel), "utf8")
      } catch {
        return null
      }

      const url = new URL(page.url, siteConfig.url).toString()
      return [
        `# ${page.data.title}`,
        page.data.description ? `> ${page.data.description}` : null,
        `Source: ${url}`,
        "",
        stripFrontmatter(raw),
      ]
        .filter(Boolean)
        .join("\n")
    })
  )

  // The standalone /why page sits outside the docs tree, so it is read
  // directly and placed first. It is the page a model should see before any
  // of the reference material.
  const whyRaw = await fs
    .readFile(path.join(PAGES_DIR, "why.mdx"), "utf8")
    .catch(() => null)
  const why = whyRaw
    ? `# The pipeline before the commit\nSource: ${new URL("/why", siteConfig.url).toString()}\n\n${stripFrontmatter(whyRaw)}`
    : null

  const text = `# ${siteConfig.name} — full documentation

> ${siteConfig.description}

Generated from ${siteConfig.url}. Index: ${new URL("/llms.txt", siteConfig.url).toString()}

${[why, ...sections].filter(Boolean).join("\n\n---\n\n")}
`

  return new Response(text, {
    headers: { "Content-Type": "text/plain; charset=utf-8" },
  })
}
