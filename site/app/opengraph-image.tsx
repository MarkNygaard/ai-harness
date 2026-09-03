import { ImageResponse } from "next/og"

import { siteConfig } from "@/lib/config"

export const size = { width: 1200, height: 630 }
export const contentType = "image/png"
export const alt = siteConfig.name

// Static export: the card is rendered once at build time, not per request.
export const dynamic = "force-static"

/**
 * Social cards are rendered at build time from `siteConfig`, not drawn by hand.
 * When the project is renamed, every card regenerates on the next build --
 * there are no PNGs to re-cut.
 */
export default function OpengraphImage() {
  return new ImageResponse(
    (
      <div
        style={{
          width: "100%",
          height: "100%",
          display: "flex",
          flexDirection: "column",
          justifyContent: "center",
          padding: "80px",
          background: "#0b0b0e",
          color: "#fafafa",
          fontFamily: "sans-serif",
        }}
      >
        <div style={{ fontSize: 76, fontWeight: 700, letterSpacing: "-0.03em" }}>
          {siteConfig.name}
        </div>
        <div
          style={{
            marginTop: 28,
            fontSize: 34,
            lineHeight: 1.35,
            color: "#a1a1aa",
            maxWidth: 900,
          }}
        >
          {siteConfig.description}
        </div>
      </div>
    ),
    size
  )
}
