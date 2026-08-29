/**
 * How strong a password looks.
 *
 * **What this can and cannot tell you.** It estimates how much guessing a
 * password would cost, and it catches the shapes that make that estimate a lie
 * — runs, repeats, a name typed back at you. It does *not* know whether the
 * password has appeared in a breach, so `strong` means "not obviously cheap to
 * guess", never "safe". The labels are worded to promise only that.
 *
 * The server is the authority on what is *allowed* (a minimum length); this is
 * only advice, and a `fair` password still submits.
 */

export type Strength = "short" | "weak" | "fair" | "good" | "strong";

export interface PasswordScore {
  strength: Strength;
  /** 0–4, for the meter. `short` is 0. */
  score: number;
  label: string;
  /** The most useful single thing to do next, or `null` when it is strong. */
  hint: string | null;
}

/** Sequences worth spotting: a run through any of these is nearly free to guess. */
const RUNS = [
  "abcdefghijklmnopqrstuvwxyz",
  "0123456789",
  "qwertyuiop",
  "asdfghjkl",
  "zxcvbnm",
];

/** The words people reach for first. Not a breach list — just the obvious ones. */
const OBVIOUS = [
  "password",
  "passwort",
  "kodeord",
  "adgangskode",
  "letmein",
  "welcome",
  "admin",
  "harness",
  "changeme",
  "secret",
  "qwerty",
  "iloveyou",
  "dragon",
  "monkey",
];

/** Size of the alphabet an attacker would have to search. */
function poolSize(pw: string): number {
  let pool = 0;
  if (/[a-z]/.test(pw)) pool += 26;
  if (/[A-Z]/.test(pw)) pool += 26;
  if (/[0-9]/.test(pw)) pool += 10;
  // Everything else — punctuation, spaces, anything non-ASCII.
  if (/[^a-zA-Z0-9]/.test(pw)) pool += 33;
  return Math.max(pool, 1);
}

/**
 * Length after discounting the parts that carry no surprise.
 *
 * `aaaaaaaaaaaa` is twelve characters and one guess; `abcdefghijkl` is twelve
 * characters and a starting letter. Counting them at face value is what makes
 * naive meters call both of them strong.
 */
function effectiveLength(pw: string): number {
  const lower = pw.toLowerCase();
  let effective = 0;
  let i = 0;
  while (i < lower.length) {
    let run = 1;
    // A repeated character: the second onward are nearly free.
    while (i + run < lower.length && lower[i + run] === lower[i]) run += 1;
    if (run > 1) {
      effective += 1 + (run - 1) * 0.2;
      i += run;
      continue;
    }
    // A run along a keyboard row or the alphabet, forwards or backwards.
    let seq = 1;
    for (const line of RUNS) {
      const at = line.indexOf(lower[i]);
      if (at < 0) continue;
      let forward = 1;
      while (
        at + forward < line.length &&
        lower[i + forward] === line[at + forward]
      ) {
        forward += 1;
      }
      let back = 1;
      while (at - back >= 0 && lower[i + back] === line[at - back]) back += 1;
      seq = Math.max(seq, forward, back);
    }
    if (seq >= 3) {
      effective += 1 + (seq - 1) * 0.25;
      i += seq;
      continue;
    }
    effective += 1;
    i += 1;
  }
  return effective;
}

/** Roughly how many bits of guessing this would cost. */
function bits(pw: string): number {
  return effectiveLength(pw) * Math.log2(poolSize(pw));
}

/** Whether the password is mostly something the person already told us. */
function echoesIdentity(pw: string, identity: string[]): boolean {
  const lower = pw.toLowerCase();
  return identity.some((raw) => {
    // The local part of an email, or a first name — the parts people reuse.
    const parts = raw
      .toLowerCase()
      .split(/[@.\s_-]+/)
      .filter((p) => p.length >= 4);
    return parts.some((p) => lower.includes(p));
  });
}

function containsObvious(pw: string): boolean {
  const lower = pw.toLowerCase();
  return OBVIOUS.some((word) => lower.includes(word));
}

/**
 * Score `password`, given the server's minimum and anything the person has
 * already typed about themselves (their name and email) so it can say when the
 * password is just those again.
 */
export function scorePassword(
  password: string,
  minLength: number,
  identity: string[] = [],
): PasswordScore {
  if (password.length === 0) {
    return { strength: "short", score: 0, label: "", hint: null };
  }
  if (password.length < minLength) {
    return {
      strength: "short",
      score: 0,
      label: "Too short",
      hint: `At least ${minLength} characters.`,
    };
  }

  let estimate = bits(password);
  let hint: string | null = null;

  // These are not small penalties, because they are not small problems: a
  // password an attacker would try in the first thousand guesses is weak
  // however long it is.
  if (echoesIdentity(password, identity)) {
    estimate = Math.min(estimate, 28);
    hint = "This is mostly your own name or email — try something unrelated.";
  } else if (containsObvious(password)) {
    estimate = Math.min(estimate, 28);
    hint = "This contains a very common word.";
  }

  if (estimate < 45) {
    return {
      strength: "weak",
      score: 1,
      label: "Weak",
      hint: hint ?? "Longer is what helps most — try a few unrelated words.",
    };
  }
  if (estimate < 60) {
    return {
      strength: "fair",
      score: 2,
      label: "Fair",
      hint:
        hint ?? "A few more characters would make this much harder to guess.",
    };
  }
  if (estimate < 80) {
    return {
      strength: "good",
      score: 3,
      label: "Good",
      hint: hint,
    };
  }
  return { strength: "strong", score: 4, label: "Strong", hint: null };
}
