import { useNavigate } from "react-router-dom";
import { Sidebar, type SidebarSection } from "@/components/Sidebar";
import { TopBar } from "@/components/TopBar";
import { PaletteFab } from "@/components/PaletteFab";

export type NavKey = "runs" | "editor" | "credentials" | "overview" | "worktrees" | "tasks";

function sections(active: NavKey): SidebarSection[] {
  const item = (id: NavKey, label: string, href: string) => ({
    id,
    label,
    href,
    active: id === active,
  });
  return [
    {
      label: "Operations",
      items: [
        item("runs", "Runs", "/"),
        item("editor", "Editor", "/editor"),
        item("tasks", "Tasks", "/tasks"),
      ],
    },
    {
      label: "System",
      items: [
        item("credentials", "Credentials", "/credentials"),
        item("overview", "Overview", "/overview"),
        item("worktrees", "Worktrees", "/worktrees"),
      ],
    },
  ];
}

/** Shared application shell: sidebar nav + top bar + scrollable content. */
export function AppShell({
  active,
  breadcrumb,
  actions,
  searchPlaceholder,
  children,
}: {
  active: NavKey;
  breadcrumb: { label: string; href?: string; current?: boolean }[];
  actions?: React.ReactNode;
  searchPlaceholder?: string;
  children: React.ReactNode;
}) {
  const navigate = useNavigate();
  return (
    <div className="grid grid-cols-[240px_1fr] h-screen overflow-hidden">
      <Sidebar
        env="local"
        sections={sections(active)}
        onItemClick={(id) => {
          const href = sections(active)
            .flatMap((s) => s.items)
            .find((i) => i.id === id)?.href;
          if (href) navigate(href);
        }}
      />
      <main className="flex flex-col min-h-0 min-w-0">
        <TopBar breadcrumb={breadcrumb} searchPlaceholder={searchPlaceholder} actions={actions} />
        <div className="flex-1 overflow-auto min-h-0">{children}</div>
      </main>
      <PaletteFab />
    </div>
  );
}
