import { useId, useState } from "react";
import { IconEye, IconEyeOff } from "@tabler/icons-react";
import { scorePassword } from "@/lib/password";
import type { PasswordScore } from "@/lib/password";

const inputCls =
  "h-9 w-full rounded-md border border-input bg-transparent px-2.5 pr-9 text-[13px] outline-none focus:ring-2 focus:ring-ring";

/** Weak through strong. `short` shares weak's colour — both mean "not yet". */
const BAR = [
  "bg-muted",
  "bg-status-failed",
  "bg-status-running",
  "bg-status-running",
  "bg-status-success",
] as const;

const TEXT = [
  "text-muted-foreground",
  "text-status-failed",
  "text-status-running",
  "text-status-running",
  "text-status-success",
] as const;

/** Four segments, filled to the score. */
function Meter({ score }: { score: PasswordScore }) {
  return (
    <div
      className="flex gap-1"
      role="img"
      aria-label={score.label ? `Password strength: ${score.label}` : undefined}
    >
      {[1, 2, 3, 4].map((step) => (
        <span
          key={step}
          className={`h-1 flex-1 rounded-full ${
            score.score >= step ? BAR[score.score] : "bg-muted"
          }`}
        />
      ))}
    </div>
  );
}

function Reveal({ shown, onToggle }: { shown: boolean; onToggle: () => void }) {
  return (
    <button
      type="button"
      onClick={onToggle}
      // Not a tab stop: someone filling this form with the keyboard is typing a
      // password, not looking for a button between the two fields.
      tabIndex={-1}
      aria-label={shown ? "Hide password" : "Show password"}
      title={shown ? "Hide" : "Show"}
      className="absolute right-1.5 top-1/2 -translate-y-1/2 rounded p-1 text-muted-foreground hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring"
    >
      {shown ? (
        <IconEyeOff className="size-4" />
      ) : (
        <IconEye className="size-4" />
      )}
    </button>
  );
}

/**
 * A password field you can read back, check against itself, and judge.
 *
 * Three things that belong together: **reveal**, because a password you cannot
 * see is one you cannot check you typed; **confirm**, because the cost of a
 * typo here is a lockout; and a **strength meter**, because the minimum length
 * the server enforces is a floor, not advice.
 *
 * The meter is advisory — `onValid` reports only what the server would accept
 * (long enough, and matching), so a `Fair` password still submits. A meter that
 * blocked would be making a promise it cannot keep, since it does not know
 * whether the password has been breached.
 */
export function PasswordField({
  value,
  onChange,
  minLength,
  identity = [],
  confirm = true,
  label = "Password",
  autoComplete = "new-password",
  onValidChange,
}: {
  value: string;
  onChange: (value: string) => void;
  minLength: number;
  /** Name and email, so the meter can say when the password is just those. */
  identity?: string[];
  /** A second field. Off for a sign-in form, where there is nothing to confirm. */
  confirm?: boolean;
  label?: string;
  autoComplete?: string;
  /** Whether the pair is currently submittable. */
  onValidChange?: (valid: boolean) => void;
}) {
  const [shown, setShown] = useState(false);
  const [second, setSecond] = useState("");
  const [touchedSecond, setTouchedSecond] = useState(false);
  const id = useId();

  const score = scorePassword(value, minLength, identity);
  const longEnough = value.length >= minLength;
  const matches = !confirm || second === value;
  const valid = longEnough && matches;

  // Report upward without an effect: this is derived, and an effect here would
  // fire a render late and let a stale value be submitted.
  const [lastReported, setLastReported] = useState<boolean | null>(null);
  if (onValidChange && lastReported !== valid) {
    setLastReported(valid);
    onValidChange(valid);
  }

  const mismatch = confirm && touchedSecond && second.length > 0 && !matches;

  return (
    <div className="flex flex-col gap-2">
      <label className="flex flex-col gap-1" htmlFor={`${id}-pw`}>
        <span className="text-[11px] font-medium text-muted-foreground">
          {label}
        </span>
        <div className="relative">
          <input
            id={`${id}-pw`}
            className={inputCls}
            type={shown ? "text" : "password"}
            autoComplete={autoComplete}
            value={value}
            onChange={(e) => onChange(e.target.value)}
            required
          />
          <Reveal shown={shown} onToggle={() => setShown((v) => !v)} />
        </div>
      </label>

      {value.length > 0 && (
        <div className="flex flex-col gap-1">
          <Meter score={score} />
          <div className="flex items-baseline justify-between gap-2">
            <span className={`text-[10px] ${TEXT[score.score]}`}>
              {score.label}
            </span>
            <span className="text-[10px] text-muted-foreground">
              {value.length} characters
            </span>
          </div>
          {score.hint && (
            <span className="text-[10px] text-muted-foreground">
              {score.hint}
            </span>
          )}
        </div>
      )}

      {confirm && (
        <label className="flex flex-col gap-1" htmlFor={`${id}-confirm`}>
          <span className="text-[11px] font-medium text-muted-foreground">
            Confirm password
          </span>
          <div className="relative">
            <input
              id={`${id}-confirm`}
              className={inputCls}
              // Follows the reveal above: checking one and not the other would
              // defeat the point of having a second field.
              type={shown ? "text" : "password"}
              autoComplete={autoComplete}
              value={second}
              onChange={(e) => setSecond(e.target.value)}
              onBlur={() => setTouchedSecond(true)}
              required
            />
            <Reveal shown={shown} onToggle={() => setShown((v) => !v)} />
          </div>
          {mismatch && (
            <span className="text-[10px] text-destructive">
              Those do not match.
            </span>
          )}
        </label>
      )}
    </div>
  );
}
