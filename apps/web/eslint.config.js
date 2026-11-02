import reactHooks from "eslint-plugin-react-hooks";
import root from "../../eslint.config.js";

export default [
  ...root,
  {
    files: ["**/*.{ts,tsx}"],
    plugins: { "react-hooks": reactHooks },
    rules: {
      "react-hooks/rules-of-hooks": "error",
      "react-hooks/exhaustive-deps": "warn",
      // Confirmations are framework dialogs (deleteAction, VeitDeleteButton), not browser popups.
      "no-alert": "error",
      "no-restricted-imports": [
        "error",
        {
          paths: [
            {
              name: "@veit/react-dialog",
              importNames: ["VeitDialog"],
              message: "VeitOverlayEditDialog oder VeitOverlayDialog mit useManagedOverlayDialogBinding verwenden.",
            },
          ],
          patterns: [
            {
              group: ["@radix-ui/*"],
              message: "Radix nur über die Veit-Bausteine aus @veit/react-controls und @veit/react-dialog.",
            },
          ],
        },
      ],
    },
  },
];
