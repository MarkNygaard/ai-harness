import type { Config } from "tailwindcss";

const config: Config = {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        // ── home-ops-agent semantic tokens ──────────────────────────────
        background: "var(--background)",
        foreground: "var(--foreground)",
        card: {
          DEFAULT: "var(--card)",
          foreground: "var(--card-foreground)",
        },
        popover: {
          DEFAULT: "var(--popover)",
          foreground: "var(--popover-foreground)",
        },
        primary: {
          DEFAULT: "var(--primary)",
          foreground: "var(--primary-foreground)",
        },
        secondary: {
          DEFAULT: "var(--secondary)",
          foreground: "var(--secondary-foreground)",
        },
        muted: {
          DEFAULT: "var(--muted)",
          foreground: "var(--muted-foreground)",
        },
        accent: {
          DEFAULT: "var(--accent)",
          foreground: "var(--accent-foreground)",
          orange: "var(--accent-orange)",
          "orange-light": "var(--accent-orange-light)",
          "orange-foreground": "var(--accent-orange-foreground)",
        },
        destructive: "var(--destructive)",
        border: "var(--border)",
        input: "var(--input)",
        ring: "var(--ring)",
        status: {
          running: "var(--status-running)",
          success: "var(--status-success)",
          failed: "var(--status-failed)",
          skipped: "var(--status-skipped)",
          pending: "var(--status-pending)",
        },

        // ── Legacy tokens (older dashboard components) ───────────────────
        bg: "var(--bg)",
        "bg-1": "var(--bg-1)",
        "bg-2": "var(--bg-2)",
        "bg-3": "var(--bg-3)",
        line: "var(--line)",
        "line-2": "var(--line-2)",
        "line-3": "var(--line-3)",
        ink: "var(--ink)",
        "ink-2": "var(--ink-2)",
        "ink-3": "var(--ink-3)",
        "ink-4": "var(--ink-4)",
        rust: "var(--rust)",
        "rust-deep": "var(--rust-deep)",
        danger: "var(--danger)",
        warn: "var(--warn)",
        ok: "var(--ok)",
        moss: "var(--moss)",
        sand: "var(--sand)",
        plum: "var(--plum)",
        sky: "var(--sky)",
      },
      borderRadius: {
        lg: "var(--radius)",
        md: "calc(var(--radius) * 0.8)",
        sm: "calc(var(--radius) * 0.6)",
      },
      fontFamily: {
        sans: ["Geist Variable", "Inter", "ui-sans-serif", "system-ui", "sans-serif"],
        mono: ["Geist Mono Variable", "ui-monospace", "JetBrains Mono", "Menlo", "monospace"],
        serif: ["Instrument Serif", "Georgia", "serif"],
      },
    },
  },
  plugins: [],
};

export default config;
