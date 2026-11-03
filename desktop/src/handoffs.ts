// Shared handoff DTOs preserve task identity, provenance, and the inspected revision across providers.
import type { Agent } from "./store";
export interface TaskSource { lane: string; provider: Agent; session: string; checkout: string; wt?: string }
export interface TaskNotes { decisions: string; completed: string; remaining: string; blockers: string; next_action: string }
export interface TaskSaved { revision: number; at: string; source: TaskSource; branch: string; head: string; incomplete: boolean; text: string }
export interface Continuation { id: string; revision: number; source: TaskSource; destination: TaskSource; prompt: string }
export interface TaskEvent { sequence: number; task: string; revision: number; at: string; source: TaskSource; kind: string; summary: string; next_action: string }
export interface SharedTask { id: string; revision: number; owner: TaskSource; objective: string; notes: TaskNotes; incomplete: boolean; continuation?: Continuation }
export interface TaskView {
  task?: SharedTask; saved?: TaskSaved; events: TaskEvent[]; truncated: boolean;
  current_branch: string; current_head: string; checkout_exists: boolean; ownership_error?: string;
  guidance: { status: string; mechanism: string; version: string; hash: string };
}
/** Changing repositories makes saved notes stale evidence; it never authorizes a reset. */
export function repositoryChanged(view: TaskView): boolean {
  return !!view.saved && (view.saved.branch !== view.current_branch || view.saved.head !== view.current_head);
}
