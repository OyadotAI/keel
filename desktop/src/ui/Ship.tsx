// The chat's one door to a pull request: a single button that does the git for you.
import { useState } from "react";
import { useStore } from "../store";
import { usePrRun, useReview } from "../prereview";
import { ship, STEP_WORDS, type ShipStep } from "../ship";
import { Working } from "./Working";
import type { Endpoint } from "../api";

/** Shown once the lane has done work and is idle, until a review or pull request takes its place. */
export function ShipCard({ lane, ep, wt }: { lane: string; ep: Endpoint; wt?: string }) {
  const l = useStore((s) => s.lanes[lane]);
  const run = usePrRun(ep, wt), review = useReview(lane, ep, wt);
  const [step, setStep] = useState<ShipStep>();
  const [error, setError] = useState<string>();
  if (!l?.git?.is_repo || l.running || l.submitting || run || review?.review) return null;
  const worked = l.git.changes.length > 0 || l.conv.turns.some((t) => t.commit || t.files.length);
  if (!worked && !step) return null;
  const go = async () => {
    setError(undefined);
    try {
      if (!(await ship(lane, l, ep, setStep))) setError("Couldn’t start. Try again in a moment.");
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setStep(undefined);
    }
  };
  return (
    <div className="strip review-strip ship-card" role="region" aria-label="Share your changes">
      <div className="ship-row">
        {step ? <><Working /><span>{STEP_WORDS[step]}</span></> : <span>{error ? `Didn’t work: ${error}` : "Happy with it? Share it as a pull request."}</span>}
        <span className="spacer" />
        {!step && <button className="primary" onClick={() => void go()}>{error ? "Try again" : "Open pull request"}</button>}
      </div>
    </div>
  );
}
