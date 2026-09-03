import { source } from "@/lib/source"

export type OrderedPage = ReturnType<typeof source.getPages>[number]

export type DocsSection = {
  title: string
  pages: OrderedPage[]
}

type TreeNode = {
  type: string
  name?: unknown
  url?: string
  index?: { url?: string }
  children?: TreeNode[]
}

function label(name: unknown): string {
  return typeof name === "string" ? name : String(name ?? "")
}

/**
 * Documentation grouped and ordered the way the sidebar presents it.
 *
 * `source.getPages()` returns a flat list in filesystem order, which is
 * alphabetical and puts a folder's own index page in with the top-level pages.
 * The page tree already encodes the `meta.json` ordering and the folder
 * structure, so reading order comes from there instead — it is the same order a
 * human reads the sidebar in, which is the order a model should receive too.
 */
export function docsSections(): DocsSection[] {
  const byUrl = new Map(source.getPages().map((p) => [p.url, p]))
  const tree = source.pageTree as unknown as TreeNode
  const sections: DocsSection[] = []
  const root: DocsSection = { title: "Getting started", pages: [] }

  for (const node of tree.children ?? []) {
    if (node.type === "page" && node.url) {
      const page = byUrl.get(node.url)
      if (page) root.pages.push(page)
      continue
    }

    if (node.type === "folder") {
      const section: DocsSection = { title: label(node.name), pages: [] }
      const indexUrl = node.index?.url
      if (indexUrl) {
        const page = byUrl.get(indexUrl)
        if (page) section.pages.push(page)
      }
      for (const child of node.children ?? []) {
        if (child.type !== "page" || !child.url || child.url === indexUrl) continue
        const page = byUrl.get(child.url)
        if (page) section.pages.push(page)
      }
      if (section.pages.length > 0) sections.push(section)
    }
  }

  return root.pages.length > 0 ? [root, ...sections] : sections
}

/** Every page, flattened back out but still in reading order. */
export function docsInOrder(): OrderedPage[] {
  return docsSections().flatMap((section) => section.pages)
}
