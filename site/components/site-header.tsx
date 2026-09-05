"use client"

import Link from "next/link"
import { usePathname } from "next/navigation"
import { GithubIcon } from "lucide-react"

import { siteConfig } from "@/lib/config"
import { ModeToggle } from "@/components/mode-toggle"
import { SearchTrigger } from "@/components/search-trigger"

/**
 * The single header for the whole site. Mounted once in the `(app)` layout so
 * the landing page and the docs share it -- the docs highlight their nav item
 * rather than swapping in different chrome.
 *
 * There is no wordmark: home is just the first nav item, at the same weight as
 * the rest, and `siteConfig.navItems` is the whole menu.
 */
export function SiteHeader() {
  const pathname = usePathname()

  // Only the most specific match is current. `/docs/deploy` is under both
  // "Docs" and "Deploy"; without picking the longest match, two items light up
  // and `aria-current="page"` lands on both, which is wrong on either count.
  const currentHref = siteConfig.navItems
    .filter(
      (item) => pathname === item.href || pathname.startsWith(`${item.href}/`)
    )
    .reduce<string | undefined>(
      (best, item) =>
        best === undefined || item.href.length > best.length ? item.href : best,
      undefined
    )

  return (
    <header className="sticky top-0 z-50 w-full border-b border-border bg-background/80 backdrop-blur">
      <div className="mx-auto flex h-14 w-full items-center px-6">
        <nav className="flex items-center gap-5 text-sm">
          {siteConfig.navItems.map((item) => {
            const active = item.href === currentHref
            return (
              <Link
                key={item.href}
                href={item.href}
                aria-current={active ? "page" : undefined}
                className={
                  // Grid so both copies share one cell. The hidden bold copy
                  // sets the width, which stops the row shuffling sideways
                  // when the active item turns semibold on navigation.
                  active
                    ? "grid font-semibold text-foreground"
                    : "grid text-muted-foreground transition-colors hover:text-foreground"
                }
              >
                <span
                  aria-hidden
                  className="invisible col-start-1 row-start-1 font-semibold"
                >
                  {item.label}
                </span>
                <span className="col-start-1 row-start-1">{item.label}</span>
              </Link>
            )
          })}
        </nav>
        <div className="ml-auto flex items-center gap-2">
          <SearchTrigger />
          <span aria-hidden className="hidden h-4 w-px bg-border sm:block" />
          <a
            href={siteConfig.links.github}
            target="_blank"
            rel="noreferrer"
            aria-label={`${siteConfig.name} on GitHub`}
            className="inline-flex size-8 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-accent hover:text-accent-foreground"
          >
            <GithubIcon className="size-4" />
          </a>
          <ModeToggle />
        </div>
      </div>
    </header>
  )
}
