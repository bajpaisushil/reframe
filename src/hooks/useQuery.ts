// React bindings for the shared query cache (src/lib/queryClient.ts).
//
//   const { data, error, isLoading, isFetching, refresh } = useQuery("workspaces", api.listWorkspaces);
//   const create = useMutation(api.createWorkspace, { invalidates: ["workspaces", "activity"] });
//
// Components sharing a key share one request and one cached result; responses that arrive after a
// newer request started are dropped, and unmounted components never receive updates.

import { useCallback, useEffect, useLayoutEffect, useRef, useState, useSyncExternalStore } from "react";
import { queryClient, type QueryState } from "@/lib/queryClient";

export interface QueryOptions {
  /** Skip fetching until true (data already cached is still returned). */
  enabled?: boolean;
  /** Cached data younger than this (ms) is reused on mount without a request. */
  staleTime?: number;
  /** Refetch stale data when the window regains focus. */
  refetchOnFocus?: boolean;
}

export interface QueryResult<T> extends QueryState<T> {
  /** No data yet and a first load is pending. */
  isLoading: boolean;
  /** Force a fresh request; resolves with the data or `undefined` on failure. Never rejects. */
  refresh: () => Promise<T | undefined>;
}

let focusListenerInstalled = false;

/** One app-wide listener: refetch watched, stale queries when the user comes back. */
function ensureFocusListener(): void {
  if (focusListenerInstalled || typeof window === "undefined") return;
  focusListenerInstalled = true;
  window.addEventListener("focus", () => queryClient.refetchOnFocus());
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "visible") queryClient.refetchOnFocus();
  });
}

export function useQuery<T>(key: string, fetcher: () => Promise<T>, options: QueryOptions = {}): QueryResult<T> {
  const { enabled = true, staleTime = 0, refetchOnFocus = false } = options;

  // Always call the latest fetcher without re-subscribing when its identity changes.
  const fetcherRef = useRef(fetcher);
  useLayoutEffect(() => {
    fetcherRef.current = fetcher;
  });
  const run = useCallback(() => fetcherRef.current(), []);

  const subscribe = useCallback((listener: () => void) => queryClient.subscribe(key, listener), [key]);
  const getSnapshot = useCallback(() => queryClient.getState<T>(key), [key]);
  const state = useSyncExternalStore(subscribe, getSnapshot, getSnapshot);

  useEffect(() => {
    queryClient.configure(key, run, { staleTime, refetchOnFocus });
    if (refetchOnFocus) ensureFocusListener();
    if (enabled) void queryClient.fetch(key, run, { staleTime });
  }, [key, enabled, staleTime, refetchOnFocus, run]);

  const refresh = useCallback(() => queryClient.fetch(key, run, { force: true }), [key, run]);

  const isLoading = state.data === undefined && (state.status === "loading" || (enabled && state.status === "idle"));
  return { ...state, isLoading, refresh };
}

export type MutationStatus = "idle" | "pending" | "success" | "error";

export interface MutationOptions<TVars, TResult> {
  onSuccess?: (result: TResult, vars: TVars) => void;
  onError?: (error: unknown, vars: TVars) => void;
  /** Query keys (or prefixes) to invalidate after success. */
  invalidates?: readonly string[] | ((result: TResult, vars: TVars) => readonly string[]);
}

export interface MutationResult<TVars, TResult> {
  status: MutationStatus;
  isPending: boolean;
  error: unknown;
  data: TResult | undefined;
  /** Run and await; rejects with the error (also stored in `error`). */
  mutateAsync: (vars: TVars) => Promise<TResult>;
  /** Fire and forget; errors are stored in `error` and passed to `onError`, never thrown. */
  mutate: (vars: TVars) => void;
  reset: () => void;
}

export function useMutation<TVars = void, TResult = unknown>(
  fn: (vars: TVars) => Promise<TResult>,
  options: MutationOptions<TVars, TResult> = {},
): MutationResult<TVars, TResult> {
  const [state, setState] = useState<{ status: MutationStatus; error: unknown; data: TResult | undefined }>({
    status: "idle",
    error: null,
    data: undefined,
  });
  const latest = useRef({ fn, options });
  useLayoutEffect(() => {
    latest.current = { fn, options };
  });
  const mounted = useRef(false);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  const callId = useRef(0);

  const mutateAsync = useCallback(async (vars: TVars): Promise<TResult> => {
    const id = ++callId.current;
    const isCurrent = () => mounted.current && id === callId.current;
    setState({ status: "pending", error: null, data: undefined });
    try {
      const result = await latest.current.fn(vars);
      const { invalidates, onSuccess } = latest.current.options;
      const keys = typeof invalidates === "function" ? invalidates(result, vars) : invalidates;
      if (keys && keys.length > 0) queryClient.invalidate(...keys);
      if (isCurrent()) setState({ status: "success", error: null, data: result });
      onSuccess?.(result, vars);
      return result;
    } catch (error) {
      if (isCurrent()) setState({ status: "error", error, data: undefined });
      latest.current.options.onError?.(error, vars);
      throw error;
    }
  }, []);

  const mutate = useCallback(
    (vars: TVars) => {
      mutateAsync(vars).catch(() => undefined);
    },
    [mutateAsync],
  );

  const reset = useCallback(() => {
    callId.current += 1;
    setState({ status: "idle", error: null, data: undefined });
  }, []);

  return { ...state, isPending: state.status === "pending", mutateAsync, mutate, reset };
}
