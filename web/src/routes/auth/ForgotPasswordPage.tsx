import { useState } from "react";
import { Link } from "react-router-dom";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { useRequestReset } from "@/lib/invites";

/**
 * Asking for a reset link.
 *
 * The answer is the same whether or not the address has an account — this page
 * is public, and one that distinguished them would be a way to learn who works
 * here.
 */
export function ForgotPasswordPage() {
  const [email, setEmail] = useState("");
  const request = useRequestReset();

  return (
    <div className="flex min-h-svh items-center justify-center p-6">
      <div className="flex w-full max-w-sm flex-col gap-4">
        <h1 className="text-center text-lg font-semibold">
          Reset your password
        </h1>
        <Card>
          <CardContent className="px-4 py-4">
            {request.isSuccess ? (
              <div className="flex flex-col gap-3 text-xs">
                <p>{request.data.message}</p>
                <p className="text-muted-foreground">
                  The link works once and expires in two hours. If it does not
                  arrive, this harness may not have mail configured — ask an
                  administrator.
                </p>
                <Button
                  size="sm"
                  variant="outline"
                  render={<Link to="/login" />}
                >
                  Back to sign in
                </Button>
              </div>
            ) : (
              <form
                className="flex flex-col gap-3"
                onSubmit={(e) => {
                  e.preventDefault();
                  request.mutate({ email: email.trim() });
                }}
              >
                <label className="flex flex-col gap-1">
                  <span className="text-[11px] font-medium text-muted-foreground">
                    Email
                  </span>
                  <input
                    className="h-9 w-full rounded-md border border-input bg-transparent px-2.5 text-[13px] outline-none focus:ring-2 focus:ring-ring"
                    type="email"
                    value={email}
                    onChange={(e) => setEmail(e.target.value)}
                    autoComplete="email"
                    required
                  />
                </label>
                {request.isError && (
                  <span className="text-[11px] text-destructive">
                    {request.error.message}
                  </span>
                )}
                <Button
                  type="submit"
                  disabled={!email.trim() || request.isPending}
                >
                  {request.isPending ? "Sending…" : "Send a link"}
                </Button>
                <Link
                  to="/login"
                  className="text-center text-[11px] text-muted-foreground underline"
                >
                  Back to sign in
                </Link>
              </form>
            )}
          </CardContent>
        </Card>
      </div>
    </div>
  );
}
