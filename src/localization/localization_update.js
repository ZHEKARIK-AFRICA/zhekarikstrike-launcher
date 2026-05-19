// src/localization/localization_update.js

let currentLanguage;

async function applyUpdatePageTranslations() {
    const h1Element = document.querySelector('h1');
    if (h1Element) {
        h1Element.textContent = await window.electronAPI.t('update_launcher_title');
    }

    const progressStatusElement = document.querySelector('#progress-status');
    if (progressStatusElement) {
        progressStatusElement.textContent = await window.electronAPI.t('updating_launcher');
    }

    const updateInfoElement = document.querySelector('.update-info');
    if (updateInfoElement) {
        updateInfoElement.textContent = await window.electronAPI.t('update_description');
    }

    const errorModalTitleElement = document.querySelector('#error-modal h2');
    if (errorModalTitleElement) {
        errorModalTitleElement.textContent = await window.electronAPI.t('error_title');
    }

    const errorMessageElement = document.querySelector('#error-message');
    if (errorMessageElement) {
        errorMessageElement.textContent = await window.electronAPI.t('error_message');
    }

    const errorModalOkElement = document.querySelector('#error-modal-ok');
    if (errorModalOkElement) {
        errorModalOkElement.textContent = await window.electronAPI.t('error_ok');
    }
    
}

// Используем window.electronAPI для изменения языка
async function handleChangeLanguage(lang) {
    currentLanguage = lang;
    await window.electronAPI.setLanguage(lang); // Устанавливаем язык через API
    applyUpdatePageTranslations();

    // Обновляем активное состояние кнопки языка
    document.querySelectorAll('.language-option').forEach(option => {
        option.classList.remove('active');
    });
    const langButton = document.getElementById(`language-${lang}`);
    if (langButton) {
        langButton.classList.add('active');
    }
}

// При загрузке страницы
document.addEventListener('DOMContentLoaded', async () => {
    currentLanguage = await window.electronAPI.getLanguage(); // Получаем сохранённый язык

    await window.electronAPI.loadTranslations(currentLanguage); // Загружаем переводы
    applyUpdatePageTranslations();

    // Устанавливаем активную кнопку языка
    const activeLangButton = document.getElementById(`language-${currentLanguage}`);
    if (activeLangButton) {
        activeLangButton.classList.add('active');
    }

    // Обработчики переключения языка
    document.getElementById('language-en').addEventListener('click', () => handleChangeLanguage('en'));
    document.getElementById('language-ru').addEventListener('click', () => handleChangeLanguage('ru'));
});