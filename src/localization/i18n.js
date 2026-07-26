import { invoke as tauriInvoke } from '@tauri-apps/api/core';

import en from './locales/en.json';
import ru from './locales/ru.json';

const dictionaries = { en, ru };

export function resolveLanguage(savedLanguage, systemLanguage = 'en') {
    if (savedLanguage === 'ru' || savedLanguage === 'en') {
        return savedLanguage;
    }
    return String(systemLanguage).toLowerCase().startsWith('ru') ? 'ru' : 'en';
}

export function createI18n({
    invoke = tauriInvoke,
    systemLanguage = globalThis.navigator?.language || 'en'
} = {}) {
    let currentLanguage = resolveLanguage(null, systemLanguage);
    let initialization;

    function t(key) {
        const dictionary = dictionaries[currentLanguage] || dictionaries.en;
        return key.split('.').reduce((value, part) => {
            if (value && Object.prototype.hasOwnProperty.call(value, part)) {
                return value[part];
            }
            return undefined;
        }, dictionary) ?? key;
    }

    async function initialize() {
        initialization ??= invoke('get_language').then((savedLanguage) => {
            currentLanguage = resolveLanguage(savedLanguage, systemLanguage);
            return currentLanguage;
        });
        return initialization;
    }

    async function setLanguage(language) {
        currentLanguage = resolveLanguage(language, systemLanguage);
        await invoke('set_language', { language: currentLanguage });
        globalThis.window?.dispatchEvent?.(
            new CustomEvent('language-changed', { detail: currentLanguage })
        );
        return currentLanguage;
    }

    return {
        initialize,
        setLanguage,
        t,
        getLanguage: () => currentLanguage
    };
}

export const i18n = createI18n();
export const t = (key) => i18n.t(key);
export const initializeLanguage = () => i18n.initialize();
export const setLanguage = (language) => i18n.setLanguage(language);
export const getLanguage = () => i18n.getLanguage();
