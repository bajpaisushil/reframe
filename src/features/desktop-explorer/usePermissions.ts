import { useCallback, useMemo, useState } from "react";
import { useToast } from "@/components/ui";
import { useQuery } from "@/hooks/useQuery";
import { api, errorMessage } from "@/lib/api";
import type { PermissionKind, PermissionStatus } from "@/types";
import { PERMISSION_NAMES, missingPermissions } from "./notices";

export interface PermissionsState {
  missing: PermissionStatus[];
  grant: (kind: PermissionKind) => void;
  granting: PermissionKind | null;
}

/**
 * OS permissions Reframe still needs (macOS; other platforms report none). Re-checked when the
 * window regains focus, since permissions are granted in System Settings.
 */
export function usePermissions(): PermissionsState {
  const toast = useToast();
  const query = useQuery("explore:permissions", api.permissions, { refetchOnFocus: true });
  const [granting, setGranting] = useState<PermissionKind | null>(null);
  const { refresh } = query;

  const grant = useCallback(
    (kind: PermissionKind) => {
      setGranting(kind);
      api
        .requestPermission(kind)
        .catch((error: unknown) => {
          toast.error(`Couldn't ask for ${PERMISSION_NAMES[kind]} access`, errorMessage(error));
        })
        .finally(() => {
          setGranting(null);
          void refresh();
        });
    },
    [refresh, toast],
  );

  const missing = useMemo(() => missingPermissions(query.data), [query.data]);
  return { missing, grant, granting };
}
