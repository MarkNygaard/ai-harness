import { useQuery } from "@tanstack/react-query";
import {
  CodeBlock,
  CodeBlockActions,
  CodeBlockContent,
  CodeBlockCopyButton,
  CodeBlockHeader,
  CodeBlockTitle,
} from "@/components/ui/code-block";
import { apiFetch } from "@/lib/api";

/**
 * How to create the Slack app: the manifest to paste, and the three steps
 * around it. The manifest comes from the server, which is what decides the
 * permissions it needs.
 */
export function SlackSetup() {
  const manifest = useQuery<string, Error>({
    queryKey: ["integrations", "slack", "manifest"],
    queryFn: async ({ signal }) =>
      (await apiFetch("/api/integrations/slack/manifest", { signal })).text(),
    refetchInterval: false,
    staleTime: Infinity,
  });

  return (
    <div className="flex flex-col gap-3 text-[12px] text-muted-foreground">
      <ol className="flex list-decimal flex-col gap-1 pl-5">
        <li>
          At{" "}
          <a
            href="https://api.slack.com/apps?new_app=1"
            target="_blank"
            rel="noreferrer"
            className="underline"
          >
            api.slack.com/apps
          </a>
          , choose <span className="font-medium">From a manifest</span>, pick
          your workspace and paste the manifest below.
        </li>
        <li>
          Install the app to the workspace. A private channel also needs the app
          invited to it; public channels work without.
        </li>
        <li>
          Copy the <span className="font-medium">Bot User OAuth Token</span>{" "}
          from OAuth &amp; Permissions into the field below.
        </li>
      </ol>
      {manifest.isError ? (
        <p className="text-destructive">{manifest.error.message}</p>
      ) : (
        manifest.data && (
          <CodeBlock size="sm">
            <CodeBlockHeader>
              <CodeBlockTitle>Slack app manifest</CodeBlockTitle>
              <CodeBlockActions>
                <CodeBlockCopyButton value={manifest.data} />
              </CodeBlockActions>
            </CodeBlockHeader>
            <CodeBlockContent className="max-h-64">
              {manifest.data}
            </CodeBlockContent>
          </CodeBlock>
        )
      )}
    </div>
  );
}
