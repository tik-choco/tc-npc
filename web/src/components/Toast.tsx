import { CheckCircle2, XCircle } from "lucide-preact";
import "../styles/components.css";

export interface ToastState {
  kind: "success" | "error";
  message: string;
}

export function Toast({ toast }: { toast: ToastState }) {
  const Icon = toast.kind === "success" ? CheckCircle2 : XCircle;
  return (
    <div class={`toast toast--${toast.kind}`} role="status">
      <Icon size={16} />
      <span>{toast.message}</span>
    </div>
  );
}
