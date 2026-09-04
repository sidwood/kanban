import pluginVue from 'eslint-plugin-vue'
import tseslint from 'typescript-eslint'

// eslint-plugin-vue exports its flat presets as array-like objects;
// Array.from turns them into plain arrays tseslint.config accepts.
// The TypeScript preset comes first because its base entry carries no
// file filter; the Vue entries then restore the SFC parser for .vue.
const vue = Array.from(pluginVue.configs['flat/recommended'])

export default tseslint.config(
  // src-tauri holds Rust and Tauri's build artifacts, neither of
  // which belongs to the web lint.
  { ignores: ['dist/', 'src-tauri/'] },
  tseslint.configs.recommended,
  ...vue,
  {
    files: ['**/*.vue'],
    languageOptions: {
      parserOptions: { parser: tseslint.parser },
    },
  },
)
