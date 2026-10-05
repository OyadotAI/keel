// Lint for the desktop page, after agentchrome's: typescript-eslint, the rules of hooks, and size
// budgets. Formatting is not lint's job.
import js from "@eslint/js";
import globals from "globals";
import reactHooks from "eslint-plugin-react-hooks";
import tseslint from "typescript-eslint";

export default tseslint.config(
  { ignores: ["dist", "node_modules", "src-tauri"] },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    files: ["**/*.{ts,tsx}"],
    languageOptions: { globals: { ...globals.browser } },
    plugins: { "react-hooks": reactHooks },
    rules: {
      "react-hooks/rules-of-hooks": "error",
      "react-hooks/exhaustive-deps": "warn",
      "@typescript-eslint/no-unused-vars": ["error", { argsIgnorePattern: "^_", caughtErrors: "none" }],
      // Budgets: a component past these is two components. Warnings while the existing ones are
      // split; a new file that starts over them is the thing to stop.
      "max-lines": ["warn", { max: 500, skipBlankLines: true, skipComments: true }],
      "max-lines-per-function": ["warn", { max: 120, skipBlankLines: true, skipComments: true }],
      complexity: ["warn", 25],
    },
  },
  { files: ["scripts/**", "*.config.*"], languageOptions: { globals: { ...globals.node } } },
  { files: ["**/*.test.ts", "scripts/**"], rules: { "max-lines-per-function": "off" } },
);
