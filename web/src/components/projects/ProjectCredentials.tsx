import { useState } from "react";
import { IconTrash } from "@tabler/icons-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  PROJECT_CREDENTIALS,
  useDeleteProjectCredential,
  useProjectCredentials,
  useSetProjectCredential,
} from "@/lib/credentials";

/** Display metadata for each per-project credential provider. */
/** Keyed by credential *field* (field names are unique across providers). */
const PROJECT_CRED_META: Record<
  string,
  { label: string; placeholder: string; help: string; secret?: boolean }
> = {
  token: {
    label: "GitHub token",
    placeholder: "ghp_… / github_pat_…",
    help: "PAT with repo + pull-request access for this project's repo. Overrides the global GitHub token; used to clone the repo and open PRs.",
  },
  git_author_email: {
    label: "GitHub commit author email",
    placeholder: "you@users.noreply.github.com",
    secret: false,
    help: "Authors this project's PR commits with this email so platforms that validate the commit author against a GitHub account (e.g. Vercel) accept them. Overrides the global value; unset → a per-step synthetic address.",
  },
  sentry_project: {
    label: "Sentry project",
    placeholder: "webshop",
    secret: false,
    help: "The Sentry project slug this project's errors are reported under. Agents looking at Sentry for this project are limited to it. Needs Sentry connected under Settings → Integrations.",
  },
  s3_bucket: {
    label: "S3 bucket",
    placeholder: "my-company-backups",
    secret: false,
    help: "Where this project's `harness s3 upload` steps put files, instead of the global bucket. The access key is the global one under Settings → Integrations, so it must be allowed to write here.",
  },
  s3_folder: {
    label: "S3 folder",
    placeholder: "sanity/production",
    secret: false,
    help: "This project's uploads go under this folder. Unset: the global folder followed by the project's name.",
  },
};

/** A project's Credentials tab: per-project overrides of the global keys. */
export function ProjectCredentialsPanel({ project }: { project: string }) {
  const creds = useProjectCredentials(project);
  return (
    <div className="flex flex-col gap-5">
      <p className="text-sm text-muted-foreground">
        Project-scoped keys override the global ones for this project. Leave one
        unset to fall back to the global value.
      </p>
      {creds.isError && (
        <p className="text-xs text-destructive">
          Failed to load credentials: {creds.error.message}
        </p>
      )}
      {PROJECT_CREDENTIALS.map(({ provider, field }) => (
        <CredentialField
          key={`${provider}:${field}`}
          project={project}
          provider={provider}
          field={field}
          configured={
            creds.data?.some((c) => c.provider === provider && c.configured) ??
            false
          }
        />
      ))}
    </div>
  );
}

function CredentialField({
  project,
  provider,
  field,
  configured,
}: {
  project: string;
  provider: string;
  field: string;
  configured: boolean;
}) {
  const meta = PROJECT_CRED_META[field];
  const set = useSetProjectCredential(project);
  const del = useDeleteProjectCredential(project);
  const [value, setValue] = useState("");

  const save = () => {
    const v = value.trim();
    if (!v) return;
    set.mutate(
      { provider, fields: { [field]: v } },
      { onSuccess: () => setValue("") },
    );
  };

  return (
    <div className="flex flex-col gap-1">
      <div className="flex items-center gap-2">
        <span className="text-xs font-medium text-muted-foreground">
          {meta.label}
        </span>
        <Badge
          variant={configured ? "success" : "outline"}
          className="text-[10px]"
        >
          {configured ? "set" : "not set"}
        </Badge>
      </div>
      <div className="flex items-center gap-2">
        {/* These are stored secrets, never a sign-in, so the browser must not
            treat them as one. Without this the password manager pairs them
            with the nearest preceding text field -- the settings sidebar's
            search box -- and fills the saved email there, which filters the
            nav down to nothing. "new-password" rather than "off" because
            Chrome ignores "off" on fields it has decided are credentials.
            routes/credentials/parts.tsx already does this; the per-project tab
            was added later and missed it. */}
        <Input
          type={meta.secret === false ? "text" : "password"}
          name={`${provider}-${field}`}
          autoComplete={meta.secret === false ? "off" : "new-password"}
          value={value}
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && save()}
          placeholder={
            configured && meta.secret !== false
              ? "set — paste a new value to replace"
              : meta.placeholder
          }
          className="flex-1 font-mono"
        />
        <Button
          size="sm"
          onClick={save}
          disabled={!value.trim() || set.isPending}
        >
          {set.isPending ? "Saving…" : "Save"}
        </Button>
        {configured && (
          <Button
            variant="ghost"
            size="sm"
            onClick={() => del.mutate(provider)}
            disabled={del.isPending}
            title="Clear this project's override"
          >
            <IconTrash className="size-3.5" /> Clear
          </Button>
        )}
      </div>
      <span className="text-[11px] text-muted-foreground">{meta.help}</span>
      {(set.isError || del.isError) && (
        <span className="text-[11px] text-destructive">
          {set.error?.message ?? del.error?.message}
        </span>
      )}
    </div>
  );
}
