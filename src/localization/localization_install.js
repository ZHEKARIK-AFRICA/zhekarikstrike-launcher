// src/localization/localization_install.js

let currentLanguage;

function setText(selector, value) {
    const element = document.querySelector(selector);
    if (element) {
        element.textContent = value;
    }
}

async function applyInstallPageTranslations() {
    document.getElementById('zhekarik-strike-title').textContent = await window.electronAPI.t('zhekarik_strike_title');
    document.getElementById('page-title').textContent = await window.electronAPI.t('install_game');
    document.querySelector('label[for="install-path"]').textContent = await window.electronAPI.t('install_path');
    document.getElementById('install-path').placeholder = await window.electronAPI.t('install_placeholder');
    document.getElementById('start-install').textContent = await window.electronAPI.t('start_install');
    document.getElementById('cancel-install').textContent = await window.electronAPI.t('cancel_install');
    document.getElementById('install-status').textContent = await window.electronAPI.t('install_status');
    document.getElementById('error-message').textContent = await window.electronAPI.t('error_message');
    document.getElementById('error-modal-title').textContent = await window.electronAPI.t('error_title');
    document.getElementById('error-modal-ok').textContent = await window.electronAPI.t('error_modal_ok');

    // Обновляем ссылки в футере
    setText('a[href="https://zhekarik.africa/strike/stream"]', await window.electronAPI.t('links.stream'));
    setText('a[href="https://zhekarik.africa/strike/vip"]', await window.electronAPI.t('links.vip'));
    setText('a[href="https://zhekarik.africa/strike/demos"]', await window.electronAPI.t('links.demos'));
}

// Используем window.electronAPI для изменения языка
async function handleChangeLanguage(lang) {
    currentLanguage = lang;
    await window.electronAPI.setLanguage(lang); // Устанавливаем язык через API
    applyInstallPageTranslations(); // Применяем переводы

    // Обновляем активное состояние кнопок языка
    document.querySelectorAll('.language-option').forEach(option => {
        option.classList.remove('active');
    });
    document.getElementById(`language-${lang}`).classList.add('active');
}

// При загрузке страницы
document.addEventListener('DOMContentLoaded', async () => {
    currentLanguage = await window.electronAPI.getLanguage(); // Получаем сохранённый язык

    await window.electronAPI.loadTranslations(currentLanguage); // Загружаем переводы
    applyInstallPageTranslations();

    // Устанавливаем активную кнопку языка
    document.getElementById(`language-${currentLanguage}`).classList.add('active');

    // Обработчики переключения языка
    document.getElementById('language-en').addEventListener('click', () => handleChangeLanguage('en'));
    document.getElementById('language-ru').addEventListener('click', () => handleChangeLanguage('ru'));
});
