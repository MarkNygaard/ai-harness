import type { MetadataRoute } from "next"

import { siteConfig } from "@/lib/config"
import { source } from "@/lib/source"

// Static export: emitted once at build time.
export const dynamic = "force-static"

export default function sitemap(): MetadataRoute.Sitemap {
  const docs = source.getPages().map((page) => ({
    url: new URL(page.url, siteConfig.url).toString(),
    changeFrequency: "weekly" as const,
    priority: 0.8,
  }))

  return [
    {
      url: siteConfig.url,
      changeFrequency: "weekly",
      priority: 1,
    },
    {
      // Standalone, so it is not in the docs tree the loop above walks.
      url: new URL("/why", siteConfig.url).toString(),
      changeFrequency: "monthly",
      priority: 0.9,
    },
    ...docs,
  ]
}
