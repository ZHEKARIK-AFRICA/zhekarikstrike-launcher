import { describe, expect, it, vi } from 'vitest';

import { createI18n, resolveLanguage } from '../../src/localization/i18n.js';

describe('resolveLanguage', () => {
    it('uses a saved supported language', () => {
        expect(resolveLanguage('en', 'ru-RU')).toBe('en');
    });

    it('uses Russian for a Russian system locale', () => {
        expect(resolveLanguage(null, 'ru-RU')).toBe('ru');
    });

    it('uses English for any other system locale', () => {
        expect(resolveLanguage(null, 'de-DE')).toBe('en');
    });
});

describe('createI18n', () => {
    it('loads the persisted language and resolves nested keys', async () => {
        const invoke = vi.fn().mockResolvedValue('en');
        const i18n = createI18n({ invoke, systemLanguage: 'ru-RU' });

        await i18n.initialize();

        expect(i18n.t('play')).toBe('PLAY');
        expect(invoke).toHaveBeenCalledWith('get_language');
    });

    it('persists an explicitly selected language', async () => {
        const invoke = vi.fn().mockResolvedValue(null);
        const i18n = createI18n({ invoke, systemLanguage: 'en-US' });

        await i18n.setLanguage('ru');

        expect(invoke).toHaveBeenCalledWith('set_language', { language: 'ru' });
        expect(i18n.t('play')).toBe('ИГРАТЬ');
    });

    it('provides prerequisite lifecycle statuses in English and Russian', async () => {
        const keys = [
            'status.prerequisite_detecting',
            'status.prerequisite_downloading',
            'status.prerequisite_installing',
            'status.prerequisite_verifying'
        ];
        const i18n = createI18n({ invoke: vi.fn().mockResolvedValue(null), systemLanguage: 'en' });
        for (const key of keys) expect(i18n.t(key)).not.toBe(key);

        await i18n.setLanguage('ru');
        for (const key of keys) expect(i18n.t(key)).not.toBe(key);
    });
});
