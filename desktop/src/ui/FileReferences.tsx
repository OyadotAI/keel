// Draft file references stay visible and removable independently of uploaded attachments.
import { useStore } from "../store";
import { Icon } from "./icons";

/** Present only validated references; failed checks never create a misleading chip. */
export function FileReferences({ lane }: { lane: string }) {
  const references = useStore((s) => s.lanes[lane]?.references);
  const checking = useStore((s) => !!s.lanes[lane]?.referencePending);
  const status = useStore((s) => s.lanes[lane]?.referenceStatus);
  const error = useStore((s) => s.lanes[lane]?.referenceError);
  return <>
    {!!references?.length && <div className="composer-attachments composer-references" aria-label="Referenced files">{references.map((reference) => <div key={reference.path} title={reference.path}>
      <Icon name="file" size={13} /><span>{reference.path}</span><button aria-label={`Remove reference to ${reference.path}`} onClick={() => useStore.getState().removeReference(lane, reference)}>×</button>
    </div>)}</div>}
    {(checking || status) && <div className="reference-status small muted" role="status">{checking ? "Checking file reference…" : status}</div>}
    {error && <div className="reference-status small error" role="alert">{error}</div>}
  </>;
}
