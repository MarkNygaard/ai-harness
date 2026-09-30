/**
 * How the harness itself is configured — the public URL it advertises, and the
 * mail server it sends through. Administrator-only, at the route.
 */
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiJson } from "./api";

export interface ConcurrencySettings {
  /** The number run submission actually gates on, after precedence. */
  effective: number;
  /** Set here, as opposed to inherited or the built-in default. */
  stored: string | null;
  from_environment: string | null;
  /**
   * Cores the host reports. Context for choosing a number, not a limit — under
   * a CPU quota this still counts every core on the machine.
   */
  host_parallelism: number;
}

export interface GeneralSettings {
  /** What everything actually uses right now. */
  public_url: string | null;
  /** Set here, as opposed to inherited from the environment. */
  stored: string | null;
  from_environment: string | null;
  concurrency: ConcurrencySettings;
}

/**
 * Omitted fields are left alone; `null` clears one back to the environment.
 * The two forms on the page save independently, so neither may send a field it
 * isn't editing.
 */
export type GeneralInput = Partial<{
  public_url: string | null;
  max_concurrent_runs: number | null;
}>;

export interface MailSettings {
  configured: boolean;
  host: string | null;
  port: number | null;
  username: string | null;
  from: string | null;
  encryption: "starttls" | "tls" | "none";
  /** Whether a password is stored, without saying what it is. */
  password_set: boolean;
}

export function useGeneralSettings(enabled: boolean) {
  return useQuery<GeneralSettings, Error>({
    queryKey: ["settings", "general"],
    enabled,
    queryFn: ({ signal }) =>
      apiJson<GeneralSettings>("/api/settings/general", { signal }),
    retry: false,
    refetchInterval: false,
  });
}

export function useSetGeneralSettings() {
  const qc = useQueryClient();
  return useMutation<GeneralSettings, Error, GeneralInput>({
    mutationFn: (body) =>
      apiJson<GeneralSettings>("/api/settings/general", {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      }),
    onSuccess: (data) => {
      qc.setQueryData(["settings", "general"], data);
      // The callback, webhook and MCP endpoint are all built from it.
      qc.invalidateQueries({ queryKey: ["mcp", "connection"] });
      qc.invalidateQueries({ queryKey: ["linear"] });
    },
  });
}

export function useMailSettings(enabled: boolean) {
  return useQuery<MailSettings, Error>({
    queryKey: ["settings", "mail"],
    enabled,
    queryFn: ({ signal }) =>
      apiJson<MailSettings>("/api/settings/mail", { signal }),
    retry: false,
    refetchInterval: false,
  });
}

export type MailInput = Partial<{
  host: string;
  port: number;
  username: string;
  /** Omit to leave the stored one alone. */
  password: string;
  from: string;
  encryption: "starttls" | "tls" | "none";
}>;

export function useSetMailSettings() {
  const qc = useQueryClient();
  return useMutation<MailSettings, Error, MailInput>({
    mutationFn: (body) =>
      apiJson<MailSettings>("/api/settings/mail", {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      }),
    onSuccess: (data) => qc.setQueryData(["settings", "mail"], data),
  });
}

/** Send a test message to your own address. */
export function useTestMail() {
  return useMutation<{ sent: boolean; to: string }, Error, void>({
    mutationFn: () =>
      apiJson<{ sent: boolean; to: string }>("/api/settings/mail/test", {
        method: "POST",
      }),
  });
}
