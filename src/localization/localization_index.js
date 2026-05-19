let currentLanguage;

// Используем await window.electronAPI.t() вместо t()
async function applyMainPageTranslations() {
    document.querySelector('#settings-modal h2').textContent = await window.electronAPI.t('settings');
    document.getElementById('zhekarik-strike-title').textContent = await window.electronAPI.t('zhekarik_strike_title');
    document.querySelector('label[for="game-path"]').textContent = await window.electronAPI.t('game_path');
    document.querySelector('#close-settings').textContent = await window.electronAPI.t('close');
    document.querySelector('#play-button').textContent = await window.electronAPI.t('play');
    document.querySelector('label[for="launch-params"]').textContent = await window.electronAPI.t('launch_params');
    document.querySelector('label[for="clan-tag"]').textContent = await window.electronAPI.t('clan_tag');
    document.querySelector('label[for="nickname"]').textContent = await window.electronAPI.t('nickname');
    document.querySelector('#check-files').textContent = await window.electronAPI.t('check_files');
    document.querySelector('#launcher-status').textContent = await window.electronAPI.t('status_ready');
    document.querySelector('#error-message').textContent = await window.electronAPI.t('error_message');
    document.getElementById('error-modal-title').textContent = await window.electronAPI.t('error_title');
    document.getElementById('error-modal-ok').textContent = await window.electronAPI.t('error_modal_ok');


    // Update footer links
    document.querySelector('a[href="https://zhekarik.africa/stream"]').textContent = await window.electronAPI.t('links.stream');
    document.querySelector('a[href="https://zhekarik.africa/vip"]').textContent = await window.electronAPI.t('links.vip');
    document.querySelector('a[href="https://zhekarik.africa/demos"]').textContent = await window.electronAPI.t('links.demos');
}

// Используем window.electronAPI вместо локальной функции
async function handleChangeLanguage(lang) {
    currentLanguage = lang;
    await window.electronAPI.setLanguage(lang); // Меняем язык через API
    applyMainPageTranslations(); // Применяем переводы

    // Обновляем активное состояние кнопки
    document.querySelectorAll('.language-option').forEach(option => {
        option.classList.remove('active');
    });
    document.getElementById(`language-${lang}`).classList.add('active');
}

// При загрузке страницы
document.addEventListener('DOMContentLoaded', async () => {
    currentLanguage = await window.electronAPI.getLanguage(); // Получаем сохранённый язык

    await window.electronAPI.loadTranslations(currentLanguage); // Загружаем переводы через API
    applyMainPageTranslations();

    // Устанавливаем активный язык
    document.getElementById(`language-${currentLanguage}`).classList.add('active');

    // Обработчики переключения языка
    document.getElementById('language-en').addEventListener('click', () => handleChangeLanguage('en'));
    document.getElementById('language-ru').addEventListener('click', () => handleChangeLanguage('ru'));
});