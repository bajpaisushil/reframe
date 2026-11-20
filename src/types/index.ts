// API types generated from the Rust models (run `npm run bindings` after changing them).
export type * from "./generated";

/** Error object rejected by every Tauri command (see src-tauri/src/error.rs). */
export interface ApiError {
  kind:
    | "validation"
    | "notFound"
    | "conflict"
    | "invalidImport"
    | "unsupported"
    | "permissionDenied"
    | "platform"
    | "database"
    | "serialization"
    | "io"
    | "notImplemented"
    | "internal";
  message: string;
  details: string[];
}
