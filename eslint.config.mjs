import globals from 'globals';
import pluginJs from '@eslint/js';

export default [
  {
    ignores: [
      'dist/**',
      'src/main/**'
    ]
  },
  pluginJs.configs.recommended,
  {
    files: ['src/renderer/**/*.js', 'src/localization/**/*.js'],
    languageOptions: {
      ecmaVersion: 'latest',
      sourceType: 'module',
      globals: globals.browser
    }
  }
];
