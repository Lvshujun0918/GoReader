function closeMe() {
	window.parent.postMessage({ action: 'pdf-xchange.no-native-iframe.close_me' }, '*');
}

$(function () {
	$('.message').html(chrome.i18n.getMessage('NativePartNotInstalled'));
	$('.close-dialog').click(function () {
		closeMe();
	});
})