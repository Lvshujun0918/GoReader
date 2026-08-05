$(function () {
	const queryParameters = Object.fromEntries((new URLSearchParams(window.location.search)).entries());
	let { p } = queryParameters;
	const params = JSON.parse(p);

	if (params.action)
		$('.action-name').text('"' + params.action + '"')

	$('.cancel-btn').click(function () {
		chrome.runtime.sendMessage({
			type: "pdf-x-change.cancel_op",
			tab_id: params.tab_id
		}, function (msg) {
			if (chrome.runtime.lastError) {
				// Could not receive the message
				// console.error(chrome.runtime.lastError.message);
				// return;
			}
			window.close();
		});
		return false;
	});

	$('.let-it-finish-btn').click(function () {
		window.close();
	});
});