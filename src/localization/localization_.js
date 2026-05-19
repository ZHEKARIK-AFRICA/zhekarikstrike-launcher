//src/localization/localization.js

let currentLanguage;

// Функция для загрузки перевода
async function loadTranslations(lang) {
    try {
        const response = await fetch(`../src/localization/locales/${lang}.json`);
        const translations = await response.json();
        applyTranslations(translations);
    } catch (error) {
        console.error('Ошибка при загрузке переводов:', error);
    }
}

// Функция для применения перевода
function applyTranslations(translations) {
    document.querySelector('#settings-modal h2').textContent = translations.settings;
    document.querySelector('label[for="game-path"]').textContent = translations.game_path;
    document.querySelector('#close-settings').textContent = translations.close;
    document.querySelector('#play-button').textContent = translations.play;
    document.querySelector('label[for="launch-params"]').textContent = translations.launch_params;
    document.querySelector('label[for="clan-tag"]').textContent = translations.clan_tag;
    document.querySelector('label[for="nickname"]').textContent = translations.nickname;
    document.querySelector('#check-files').textContent = translations.check_files;
    document.querySelector('#launcher-status').textContent = translations.status_ready;
    document.querySelector('#error-message').textContent = translations.error_message;
    document.querySelector('#error-modal-content h2').textContent = translations.error_title;

    // Обновляем ссылки в футере
    document.querySelector('a[href="https://zhekarik.africa/stream"]').textContent = translations.links.stream;
    document.querySelector('a[href="https://zhekarik.africa/vip"]').textContent = translations.links.vip;
    document.querySelector('a[href="https://zhekarik.africa/demos"]').textContent = translations.links.demos;
}

// Функция для смены языка
function changeLanguage(lang) {
    currentLanguage = lang;
    window.electronAPI.setLanguage(lang); // Сохраняем выбранный язык в конфигурацию
    loadTranslations(lang);

    // Обновляем состояние активных кнопок
    document.querySelectorAll('.language-option').forEach(option => {
        option.classList.remove('active');
    });
    document.getElementById(`language-${lang}`).classList.add('active');
}

// Загружаем переводы при загрузке страницы
document.addEventListener('DOMContentLoaded', async () => {
    currentLanguage = await window.electronAPI.getLanguage(); // Получаем сохранённый язык

    loadTranslations(currentLanguage);

    // Устанавливаем текущий активный язык
    document.getElementById(`language-${currentLanguage}`).classList.add('active');

    // Обработчики для переключения языка
    document.getElementById('language-en').addEventListener('click', () => changeLanguage('en'));
    document.getElementById('language-ru').addEventListener('click', () => changeLanguage('ru'));
});