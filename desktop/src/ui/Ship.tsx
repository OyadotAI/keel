// The chat's one door to a pull request: a single button that does the git for you.
import { useState } from "react";
import { useStore } from "../store";
import { usePrRun, useReview } from "../prereview";
import { ship, shareKey, shareOffer, STEP_WORDS, type ShipStep } from "../ship";
import { Working } from "./Working";
import { Icon } from "./icons";
import { PrLink } from "./PreReview";
import type { Endpoint } from "../api";
import { usePlain } from "../plain";

const COPY = {
  open: { text: "Happy with it? Share it as a pull request.", button: "Open pull request" },
  update: { text: "Share your new changes.", button: "Update pull request" },
};
/** A chat lane's words: a version saved to GitHub, never a pull request. */
const PLAIN = {
  open: { text: "Happy with it? Save this version to GitHub.", button: "Save to GitHub" },
  update: { text: "New changes since your last save.", button: "Save new changes" },
};
const CLOSE = "Hide until there are new changes";

/** Shown while the lane is idle and has work its base or its pull request lacks, until it is
 *  shared or closed. A pull request already open keeps its link here after its result is closed. */
export function ShipCard({ lane, ep, wt }: { lane: string; ep: Endpoint; wt?: string }) {
  const l = useStore((s) => s.lanes[lane]);
  const run = usePrRun(ep, wt), review = useReview(lane, ep, wt);
  const [step, setStep] = useState<ShipStep>();
  const plain = usePlain(lane);
  const words = plain ? PLAIN : COPY;
  const [error, setError] = useState<string>();
  if (!l?.git?.is_repo || l.running || l.submitting || run || review?.review) return null;
  const git = l.git, offer = shareOffer(git, l.shipDismissed);
  const go = async () => {
    setError(undefined);
    try {
      // The same door for more changes: opening finds the pull request and sends it the new commits.
      if (!(await ship(lane, l, ep, setStep))) setError("Couldn’t start. Try again in a moment.");
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setStep(undefined);
    }
  };
  const close = () => { setError(undefined); useStore.getState().dismissShip(lane, shareKey(git)); };
  const card = (offer || step || error) && (
    <div className="ship-row">
      {step ? <><Working /><span>{STEP_WORDS[step]}</span></> : <span className="ship-text" title={error}>{error ? `${plain ? "Couldn’t save to GitHub" : "Didn’t work"}: ${error.split(/\n|\s+hint:/)[0].trim()}` : words[offer ?? "open"].text}</span>}
      {!step && <button className="primary" onClick={() => void go()}>{error ? "Try again" : words[offer ?? "open"].button}</button>}
      {!step && <button className="ghost review-close" aria-label={CLOSE} title={CLOSE} onClick={close}><Icon name="x" size={14} /></button>}
    </div>
  );
  if (!card && !git.pr) return null;
  return (
    <div className="strip review-strip ship-card" role="region" aria-label="Share your changes">
      {git.pr && <div className="ship-row ship-shared"><span className="small muted">{plain ? "On GitHub" : "Pull request"}</span><PrLink link={git.pr.url} /></div>}
      {card}
    </div>
  );
}
