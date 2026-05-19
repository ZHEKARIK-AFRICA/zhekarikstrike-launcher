const fs = require('fs');
const path = require('path');

let currentLanguage = 'en';
let translations = {};

// Function to load translations
async function loadTranslations(lang) {
    try {
        // Формируем полный путь к файлу локализации
        const filePath = path.join(__dirname, `../localization/locales/${lang}.json`);

        // Читаем файл синхронно (можно также использовать асинхронные методы fs, если необходимо)
        const data = fs.readFileSync(filePath, 'utf-8');

        // Парсим JSON файл
        translations = JSON.parse(data);
        currentLanguage = lang;
    } catch (error) {
        console.error('Error loading translations:', error);
    }
}

// Function to get translation by key
function t(key) {
    const keys = key.split('.');
    let value = translations;
    for (const k of keys) {
        value = value[k];
        if (value === undefined) {
            console.warn(`Translation key not found: ${key}`);
            return key;
        }
    }
    return value;
}

// Function to change the language
async function changeLanguage(lang) {
    await loadTranslations(lang);
    currentLanguage = lang;
}

module.exports = {
    loadTranslations,
    t,
    changeLanguage
};