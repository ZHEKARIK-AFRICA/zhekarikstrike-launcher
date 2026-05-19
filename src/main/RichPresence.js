// RichPresence.js

const DiscordRPC = require('discord-rpc');
const clientId = '1285625078800846941'; // Замените на ваш Client ID приложения Discord

DiscordRPC.register(clientId);

const rpc = new DiscordRPC.Client({ transport: 'ipc' });
let startTimestamp;
let connected = false;
let activityUpdater;

async function setActivity() {
    console.log('starting rpc activity')
    if (!rpc || !connected) {
        console.log('rpc not connected')
        return;
    }

    rpc.setActivity({
        details: 'ВЫНОСИТ НУБЧИКОВ )',
        startTimestamp,
        largeImageKey: 'logo', // Замените на ключ вашего изображения
        largeImageText: 'жикосик аэаэаэ)',
        buttons: [
            {
                label: 'ГО ГО ГО', // Текст кнопки
                url: 'https://zhekarik.africa' // URL кнопки
            }
        ],
        instance: false,
    });
}

function startRichPresence() {
    if (connected) {
        return; // Уже подключены
    }

    rpc.on('ready', () => {
        connected = true;
        startTimestamp = new Date();

        setActivity();

        // Обновляем активность каждые 15 секунд
        activityUpdater = setInterval(() => {
            setActivity();
        }, 15e3);
    });

    rpc.login({ clientId }).catch(console.error);

    rpc.on('error', (error) => {
        console.error('Discord RPC Error:', error);
    });

    rpc.on('disconnected', (code, reason) => {
        console.log(`Discord RPC disconnected: ${code} - ${reason}`);
        connected = false;
    });
}

function stopRichPresence() {
    if (!connected) {
        return; // Не подключены
    }

    clearInterval(activityUpdater); // Останавливаем обновление активности

    rpc.clearActivity().catch(console.error);

    rpc.destroy().then(() => {
        connected = false;
    }).catch((error) => {
        console.error('Error during RPC destroy:', error);
    });
}

module.exports = {
    startRichPresence,
    stopRichPresence,
};
