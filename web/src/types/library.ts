/**
 * The workflow library, as the harness reports it: the registry's row for each
 * workflow plus what is true of it on this install.
 */

export interface LibraryEntry {
  /** The registry's identifier. Not the local file name — they may differ. */
  slug: string;
  title: string;
  description: string;
  tags: string[];
  /** Published by the project rather than by a person. */
  official: boolean;
  /** The publisher's GitHub login. */
  publisher: string;
  /** Null when every version has been withdrawn. */
  latest_version: number | null;
  /** How many harnesses currently have it installed. */
  installs: number;
  updated_at: string;
  /** The local file stem, when this is installed here. */
  installed_as: string | null;
  /** The version on disk, which is not necessarily the latest. */
  installed_version: number | null;
  /** The registry has something newer than what is installed. */
  update_available: boolean;
}
