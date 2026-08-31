import { siteConfig } from "@/lib/config"

export function SiteFooter() {
  return (
    <footer>
      <div className="mx-auto flex w-full max-w-6xl flex-col gap-2 px-6 py-8 text-sm text-muted-foreground sm:flex-row sm:items-center sm:justify-between">
        <span>{siteConfig.name} — MIT licensed.</span>
        <span>
          The source code is available on{" "}
          <a
            className="underline underline-offset-4 hover:text-foreground"
            href={siteConfig.links.github}
            target="_blank"
            rel="noreferrer"
          >
            GitHub
          </a>
          .
        </span>
      </div>
    </footer>
  )
}
