// Keeps a terminal at the bottom of its output unless the person scrolled away from it.

/// How long after a wheel, a key or a press a scroll still counts as the person's.
export const PERSON_SCROLL_MS = 300;

/// What a terminal's scroll position should do next. xterm decides this from DOM scroll events,
/// and loses the race when a program wipes the scrollback and reprints it: agy does exactly that
/// on every resize (measured on 1.5.13: `ESC[2J ESC[3J`, then the whole conversation), a stale
/// scroll event lands at row 0, and the lane came back at the top of hours of history. So Keel
/// keeps the answer itself: a scroll the person made moves `following`, any other is undone.
export function nextFollow(following: boolean, atBottom: boolean, byPerson: boolean): { following: boolean; toBottom: boolean } {
  if (byPerson) return { following: atBottom, toBottom: false };
  return { following, toBottom: following && !atBottom };
}
