/** Dots marching a loop, like ants at work; moves only while real work is in progress, and reduced motion keeps a static mark. */
export function Working() {
  return <span className="working-animation" aria-hidden="true"><i /><i /><i /><i /><i /></span>;
}
