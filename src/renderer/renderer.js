document.addEventListener('DOMContentLoaded', () => {
    console.log('Renderer loaded, electronAPI:', window.electronAPI);

    // Обработчик для открытия внешних ссылок
    document.addEventListener('click', (event) => {
        const target = event.target.closest('a');
        
        if (target && target.href && (target.href.startsWith('http://') || target.href.startsWith('https://'))) {
            event.preventDefault();

            try {
                window.electronAPI.openExternal(target.href); // Попытка открытия ссылки
                console.log('Opening link:', target.href);
            } catch (error) {
                console.error('Failed to open external link:', error);
            }
        }
    });

    // Добавляем обработчики только если кнопки существуют
    const closeWindowButton = document.getElementById('close-window');
    if (closeWindowButton) {
        closeWindowButton.addEventListener('click', () => {
            window.electronAPI.closeWindow();
        });
    } else {
        console.error('Close window button not found');
    }

    const minimizeWindowButton = document.getElementById('minimize-window');
    if (minimizeWindowButton) {
        minimizeWindowButton.addEventListener('click', () => {
            window.electronAPI.minimizeWindow();
        });
    } else {
        console.error('Minimize window button not found');
    }
});

document.getElementById('close-window').addEventListener('click', () => {
    window.electronAPI.closeWindow();
});

document.getElementById('minimize-window').addEventListener('click', () => {
    window.electronAPI.minimizeWindow();
});

document.addEventListener('DOMContentLoaded', () => {
    const settingsButton = document.getElementById('settings-button');
    const settingsModal = document.getElementById('settings-modal');
    const closeSettingsButton = document.getElementById('close-settings');
    const deleteGameButton = document.getElementById('delete-game-button');
    const selectFolderButton = document.getElementById('select-folder-button');

    // Открытие модального окна "Настройки"
    settingsButton.addEventListener('click', () => {
        settingsModal.style.display = 'block';
    });

    // Закрытие модального окна "Настройки"
    closeSettingsButton.addEventListener('click', () => {
        settingsModal.style.display = 'none';
    });

    // Закрытие окна при клике вне его
    window.addEventListener('click', (event) => {
        if (event.target === settingsModal) {
            settingsModal.style.display = 'none';
        }
    });

});