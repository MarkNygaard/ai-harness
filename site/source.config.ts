import {
  defineCollections,
  defineConfig,
  defineDocs,
  frontmatterSchema,
} from "fumadocs-mdx/config"

export const docs = defineDocs({
  dir: "content/docs",
})

/**
 * Standalone pages that sit beside the landing page rather than inside the docs
 * tree. Authored as MDX like everything else so they stay in `llms.txt`, but
 * rendered without the docs sidebar: the reader has not decided to use the
 * product yet.
 */
export const pages = defineCollections({
  type: "doc",
  dir: "content/pages",
  // Without a schema the frontmatter types as `unknown`.
  schema: frontmatterSchema,
})

export default defineConfig({
  mdxOptions: {
    rehypeCodeOptions: {
      themes: {
        light: "github-light-default",
        dark: "vesper",
      },
    },
  },
})
