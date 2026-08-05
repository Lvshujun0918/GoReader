$(function () {
	document.title = chrome.i18n.getMessage('UpdateAppPromptTitle');
	$('.update-msg').text(chrome.i18n.getMessage('UpdateAppPromptMsg'));
	$('.link-text').text(chrome.i18n.getMessage('UpdateAppPromptLinkText'));
});
