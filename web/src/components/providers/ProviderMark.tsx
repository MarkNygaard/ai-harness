import { IconBrandGithub, IconBrandOpenai } from "@tabler/icons-react";

/**
 * Brand marks for the providers, so a row is identifiable before it is read.
 *
 * Tabler ships GitHub and OpenAI; the rest are drawn here because there is no
 * icon set that carries them. They are simplified to what survives at 16px —
 * enough to tell the rows apart at a glance, which is the whole job.
 */

const svg = {
  className: "size-4",
  viewBox: "0 0 24 24",
  fill: "none",
  stroke: "currentColor",
  strokeWidth: 1.75,
  strokeLinecap: "round" as const,
  strokeLinejoin: "round" as const,
};

/** Anthropic's burst: rays from a centre, alternating long and short. */
function ClaudeMark() {
  return (
    <svg {...svg} aria-hidden="true">
      <path d="M12 3v5M12 16v5M3 12h5M16 12h5" />
      <path d="M6.3 6.3 9 9M15 15l2.7 2.7M17.7 6.3 15 9M9 15l-2.7 2.7" />
    </svg>
  );
}

/** Cursor's isometric cube, as a hexagon with the three visible edges. */
function CursorMark() {
  return (
    <svg {...svg} aria-hidden="true">
      <path d="M12 2.5 20.5 7v10L12 21.5 3.5 17V7z" />
      <path d="M12 12 20.5 7M12 12l-8.5-5M12 12v9.5" />
    </svg>
  );
}

/** Kimi is Moonshot AI — a crescent. */
function KimiMark() {
  return (
    <svg {...svg} aria-hidden="true">
      <path d="M19 15.5A8 8 0 0 1 8.5 5a8.5 8.5 0 1 0 10.5 10.5z" />
    </svg>
  );
}

/** Linear's rounded square, with its diagonal rules. */
function LinearMark() {
  return (
    <svg {...svg} aria-hidden="true">
      <rect x="3" y="3" width="18" height="18" rx="4.5" />
      <path d="M3.6 13.2 10.8 20.4M4 8.2 15.8 20M7 4.4 19.6 17" />
    </svg>
  );
}

const MARKS: Record<string, () => React.JSX.Element> = {
  claude: ClaudeMark,
  codex: () => <IconBrandOpenai className="size-4" />,
  pi: KimiMark,
  cursor: CursorMark,
  github: () => <IconBrandGithub className="size-4" />,
  linear: LinearMark,
};

/**
 * Brand tints, kept muted enough to sit in a settings list.
 *
 * OpenAI, Cursor and GitHub have monochrome marks by design, so they inherit
 * the text colour rather than being given one that isn't theirs.
 */
const TINT: Record<string, string> = {
  claude: "text-accent-orange",
  pi: "text-violet-400",
  linear: "text-indigo-400",
};

export type ProviderStatus = "ok" | "warn" | "bad" | "off";

const DOT: Record<ProviderStatus, string> = {
  ok: "bg-status-success",
  warn: "bg-accent-orange",
  bad: "bg-status-failed",
  off: "bg-muted-foreground/40",
};

/**
 * A provider's mark with its status as a dot on the corner.
 *
 * The dot carries the state that used to be a "connected" / "not set" chip.
 * It is not the only signal — every row states its status in words underneath —
 * so nothing here depends on telling red from green.
 */
export function ProviderMark({
  provider,
  status,
}: {
  provider: string;
  status: ProviderStatus;
}) {
  const Mark = MARKS[provider];
  return (
    <span className="relative inline-flex shrink-0 items-center justify-center">
      <span className={TINT[provider] ?? "text-foreground"}>
        {Mark ? <Mark /> : <span className="size-4" />}
      </span>
      <span
        aria-hidden="true"
        className={`absolute -left-1 -top-1 size-1.5 rounded-full ring-2 ring-background ${DOT[status]}`}
      />
    </span>
  );
}
