import { createContext, useContext } from "react";

/** Per-node actions exposed to the custom node (configure opens the drawer). */
export interface EditorActions {
  onConfigure: (id: string) => void;
  onDelete: (id: string) => void;
  selectedId: string | null;
}

export const EditorActionsContext = createContext<EditorActions>({
  onConfigure: () => {},
  onDelete: () => {},
  selectedId: null,
});

export const useEditorActions = () => useContext(EditorActionsContext);
