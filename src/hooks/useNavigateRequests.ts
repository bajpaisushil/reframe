import { useEffect } from "react";
import { useToast } from "@/components/ui/Toast";
import { onNavigateRequest } from "@/lib/events";
import { parseNavigateRequest } from "@/lib/navigation";
import { navigate } from "@/lib/router";

/**
 * Follow navigation requests from the native side (`reframe://navigate`, sent by the system
 * tray): open the requested page and show any error message as a toast. Mount once, in the shell.
 */
export function useNavigateRequests(): void {
  const toast = useToast();

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;

    onNavigateRequest((payload) => {
      if (disposed) return;
      const { route, message } = parseNavigateRequest(payload);
      if (route) navigate(route);
      if (message) toast.error(message);
    })
      .then((stop) => {
        // `listen` resolves asynchronously: if the shell unmounted meanwhile, stop right away.
        if (disposed) stop();
        else unlisten = stop;
      })
      .catch((error: unknown) => {
        // Without the listener the tray can still open the window; it just cannot change pages.
        console.warn("Reframe: could not listen for navigation requests", error);
      });

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [toast]);
}
