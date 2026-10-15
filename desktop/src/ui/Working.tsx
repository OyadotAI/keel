/** CSS motion changes only while real work is in progress; reduced motion retains a static mark. */
export function Working() {
  return <span className="working-animation" aria-hidden="true"><i /><i /><i /></span>;
}
