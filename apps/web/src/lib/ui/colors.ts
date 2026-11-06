// Stable colours for things that have no colour of their own (projects,
// labels): hash the name into a small palette tuned for the dark base.

const PALETTE = [
  "#5fbfb0",
  "#c58ad8",
  "#7fb1ea",
  "#d98a3d",
  "#9fca72",
  "#e3bd55",
  "#e5675c",
  "#ad8fe0",
  "#6f9fe2",
  "#d6869e",
];

function hash(s: string): number {
  let h = 2166136261;
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  return h >>> 0;
}

export function projectColor(key: string): string {
  return PALETTE[hash(key) % PALETTE.length];
}

// A few labels mean the same thing everywhere; give them the obvious colour.
const FIXED: Record<string, string> = {
  bug: "#e5675c",
  security: "#e5675c",
  feature: "#63b780",
  enhancement: "#63b780",
  docs: "#a69c90",
  documentation: "#a69c90",
};

export function labelColor(label: string): string {
  return FIXED[label.toLowerCase()] ?? PALETTE[hash(label.toLowerCase()) % PALETTE.length];
}

// One stable tint per agent name, so each agent's claimed cards read as
// theirs on the board and on the Agents page. Salted so agent hues don't
// track project hues for the same string.
export function agentColor(name: string): string {
  return PALETTE[hash(`agent:${name.toLowerCase().trim()}`) % PALETTE.length];
}
