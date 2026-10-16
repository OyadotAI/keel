export interface GraphCommit {
  sha: string;
  parents: string[];
  refs: string[];
  author: string;
  when: number;
  subject: string;
}
interface Lane { sha: string; color: number }
export interface GraphEdge { from: number; to: number; color: number; half: "top" | "bottom" | "through" }
export interface GraphRow { commit: GraphCommit; column: number; color: number; width: number; edges: GraphEdge[] }

/** Pending parent hashes define the columns. Lines end only at actual root commits. */
export function commitGraph(commits: GraphCommit[]): GraphRow[] {
  let lanes: Lane[] = [];
  let colors = 0;
  return commits.map((commit) => {
    let column = lanes.findIndex((l) => l.sha === commit.sha);
    const incoming = column >= 0;
    if (!incoming) { column = lanes.length; lanes.push({ sha: commit.sha, color: colors++ }); }
    const color = lanes[column].color;
    const before = [...lanes];
    lanes = lanes.filter((l) => l.sha !== commit.sha);
    const parents = [...new Set(commit.parents)];
    parents.forEach((sha, i) => {
      if (!lanes.some((l) => l.sha === sha)) {
        lanes.splice(Math.min(column + i, lanes.length), 0, { sha, color: i === 0 ? color : colors++ });
      }
    });
    const edges: GraphEdge[] = [];
    before.forEach((l, from) => {
      if (l.sha === commit.sha) {
        if (incoming) edges.push({ from, to: column, color, half: "top" });
      } else edges.push({ from, to: lanes.findIndex((next) => next.sha === l.sha), color: l.color, half: "through" });
    });
    parents.forEach((sha) => {
      const to = lanes.findIndex((l) => l.sha === sha);
      edges.push({ from: column, to, color: lanes[to].color, half: "bottom" });
    });
    return { commit, column, color, width: Math.max(before.length, lanes.length), edges };
  });
}
