import { useState } from "react";
import { Tags, Trash2 } from "lucide-react";
import { SettingsShell } from "@/components/SettingsShell";
import { Card, CardContent } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import {
  useCategories,
  useDeleteCategory,
  useSaveCategory,
  type Category,
} from "@/lib/categories";

const inputCls =
  "h-8 rounded-md border border-input bg-transparent px-2.5 text-[12px] outline-none focus:ring-2 focus:ring-ring";

/** Manage the global step categories used for overview grouping + bar colours. */
export function CategoriesPage() {
  const cats = useCategories();
  return (
    <SettingsShell title="Categories">
      <div className="mx-auto flex max-w-3xl flex-col gap-5 p-6">
        <div>
          <h1 className="flex items-center gap-2 text-lg font-semibold">
            <Tags className="h-5 w-5 text-accent-orange" /> Step categories
          </h1>
          <p className="mt-1 text-sm text-muted-foreground">
            Group workflow steps (assign one per node in the editor). The run
            overview shows a time-by-category bar and colours the waterfall by
            category. Uncategorized steps keep their status colour.
          </p>
        </div>

        {cats.isError && (
          <p className="text-sm text-destructive">
            Couldn’t load categories: {cats.error.message}
          </p>
        )}

        <div className="flex flex-col gap-2">
          {(cats.data ?? []).map((c) => (
            <CategoryRow key={c.id} category={c} />
          ))}
        </div>

        <NewCategoryForm />
      </div>
    </SettingsShell>
  );
}

function Swatch({ color }: { color: string }) {
  return (
    <span
      className="size-5 shrink-0 rounded-sm border border-border"
      style={{ backgroundColor: color }}
      aria-hidden
    />
  );
}

function CategoryRow({ category }: { category: Category }) {
  const save = useSaveCategory();
  const del = useDeleteCategory();
  const [label, setLabel] = useState(category.label);
  const [color, setColor] = useState(category.color);
  const [ordinal, setOrdinal] = useState(String(category.ordinal));
  const dirty =
    label !== category.label ||
    color !== category.color ||
    ordinal !== String(category.ordinal);

  return (
    <Card>
      <CardContent className="flex items-center gap-3 py-2.5">
        <Swatch color={color} />
        <div className="flex flex-1 flex-wrap items-center gap-2">
          <input
            className={`${inputCls} w-36`}
            value={label}
            onChange={(e) => setLabel(e.target.value)}
            aria-label="Label"
          />
          <input
            className={`${inputCls} w-52 font-mono`}
            value={color}
            onChange={(e) => setColor(e.target.value)}
            aria-label="Color"
            placeholder="oklch(...) or #hex"
          />
          <input
            className={`${inputCls} w-16`}
            type="number"
            value={ordinal}
            onChange={(e) => setOrdinal(e.target.value)}
            aria-label="Ordinal"
            title="Sort order"
          />
          <span className="font-mono text-[11px] text-muted-foreground">
            {category.id}
          </span>
        </div>
        <Button
          size="sm"
          disabled={!dirty || save.isPending}
          onClick={() =>
            save.mutate({
              id: category.id,
              label: label.trim(),
              color: color.trim(),
              ordinal: Number(ordinal) || 0,
            })
          }
        >
          {save.isPending ? "Saving…" : "Save"}
        </Button>
        <Button
          variant="ghost"
          size="icon"
          aria-label="Delete category"
          title="Delete category"
          disabled={del.isPending}
          onClick={() => {
            if (window.confirm(`Delete category “${category.label}”?`))
              del.mutate(category.id);
          }}
          className="size-7 shrink-0 text-muted-foreground hover:text-destructive"
        >
          <Trash2 className="size-3.5" />
        </Button>
      </CardContent>
    </Card>
  );
}

function NewCategoryForm() {
  const save = useSaveCategory();
  const [id, setId] = useState("");
  const [label, setLabel] = useState("");
  const [color, setColor] = useState("oklch(0.7 0.06 300)");

  function submit(e: React.FormEvent) {
    e.preventDefault();
    const slug = id.trim();
    if (!slug || !label.trim()) return;
    save.mutate(
      { id: slug, label: label.trim(), color: color.trim() },
      {
        onSuccess: () => {
          setId("");
          setLabel("");
        },
      },
    );
  }

  return (
    <Card>
      <CardContent className="py-4">
        <div className="mb-3 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
          Add a category
        </div>
        <form onSubmit={submit} className="flex flex-wrap items-end gap-2">
          <label className="flex flex-col gap-1">
            <span className="text-[11px] text-muted-foreground">Id (slug)</span>
            <input
              className={`${inputCls} w-36 font-mono`}
              value={id}
              onChange={(e) => setId(e.target.value)}
              placeholder="research"
            />
          </label>
          <label className="flex flex-col gap-1">
            <span className="text-[11px] text-muted-foreground">Label</span>
            <input
              className={`${inputCls} w-40`}
              value={label}
              onChange={(e) => setLabel(e.target.value)}
              placeholder="Research"
            />
          </label>
          <label className="flex flex-col gap-1">
            <span className="text-[11px] text-muted-foreground">Color</span>
            <div className="flex items-center gap-2">
              <Swatch color={color} />
              <input
                className={`${inputCls} w-52 font-mono`}
                value={color}
                onChange={(e) => setColor(e.target.value)}
                placeholder="oklch(...) or #hex"
              />
            </div>
          </label>
          <Button
            type="submit"
            size="sm"
            disabled={save.isPending || !id.trim() || !label.trim()}
          >
            {save.isPending ? "Adding…" : "Add"}
          </Button>
          {save.isError && (
            <span className="text-xs text-destructive">
              {save.error.message}
            </span>
          )}
        </form>
      </CardContent>
    </Card>
  );
}
