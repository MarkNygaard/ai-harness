import { describe, expect, it, beforeEach } from "vitest";
import { render, screen, fireEvent, act } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TOKEN_KEY, unauthorizedEvents } from "@/lib/api";
import { TokenPrompt } from "./TokenPrompt";

function renderWithClient(ui: React.ReactElement) {
  const client = new QueryClient();
  return render(
    <QueryClientProvider client={client}>{ui}</QueryClientProvider>,
  );
}

describe("<TokenPrompt>", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it("does not render initially", () => {
    const { container } = renderWithClient(<TokenPrompt />);
    expect(container.querySelector('input[type="password"]')).toBeNull();
  });

  it("opens on unauthorized event and saves token to localStorage", () => {
    renderWithClient(<TokenPrompt />);
    act(() => {
      unauthorizedEvents.dispatchEvent(new Event("unauthorized"));
    });
    const input = screen.getByPlaceholderText(
      /bearer token/i,
    ) as HTMLInputElement;
    fireEvent.change(input, { target: { value: "secret" } });
    fireEvent.click(screen.getByText(/save/i));
    expect(localStorage.getItem(TOKEN_KEY)).toBe("secret");
  });
});
