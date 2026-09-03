import type { CSSProperties } from "react";
import { Toaster as Sonner, type ToasterProps } from "sonner";

import { useTheme } from "@/lib/theme";

/**
 * Toast host. Mount once near the root; anything can then call `toast()`.
 *
 * The registry component reads the theme from `next-themes`, which this app
 * does not use — `lib/theme` is what decides whether `.dark` is on `<html>`, so
 * it is what sonner has to follow or a toast paints in the other palette. The
 * *resolved* theme, not the preference: sonner wants a concrete light or dark,
 * and `system` is neither.
 *
 * Sonner ships its own stylesheet and colours it from these four variables, so
 * pointing them at the popover tokens is what keeps a toast looking like the
 * rest of the app in both themes.
 */
export function Toaster(props: ToasterProps) {
  const { resolved } = useTheme();

  return (
    <Sonner
      theme={resolved}
      className="toaster group"
      style={
        {
          "--normal-bg": "var(--popover)",
          "--normal-text": "var(--popover-foreground)",
          "--normal-border": "var(--border)",
          "--border-radius": "var(--radius-lg)",
        } as CSSProperties
      }
      {...props}
    />
  );
}
