// Lint the UI (plain ES modules, no build step): catches undefined names,
// duplicate keys and unreachable code that `node --check` can't see.
import globals from "globals";

export default [
  { ignores: ["ui/vendor/**", "src-tauri/**", "dev/**", "node_modules/**"] },
  {
    files: ["ui/js/**/*.js"],
    languageOptions: { ecmaVersion: 2022, sourceType: "module", globals: { ...globals.browser } },
    rules: {
      "no-undef": "error",
      "no-dupe-keys": "error",
      "no-unreachable": "error",
      "no-redeclare": "error",
      "no-const-assign": "error",
      "no-import-assign": "error",
      "no-unused-vars": ["warn", { args: "none" }],
    },
  },
];
