let gActiveDocumentType;
let gActiveTabID;

const show = (selector) => $(selector).removeClass('hidden');
const hide = (selector) => $(selector).addClass('hidden');

const Progress = {

	initialized: false,

	init() {
		if (this.initialized)
			return;

		chrome.runtime.onMessage.addListener(
			function (msg, sender, sendResponse) {
				if (chrome.runtime.lastError) {
					// Could not receive the message
					console.error(chrome.runtime.lastError.message);
					return;
				}

				if (msg) {
					if (msg.tabID === gActiveTabID) {
						if (msg.type) {
							switch (msg.type) {
								case 'pdf-x-change.progress.post':
									Progress.render(msg);
									break;
								case 'pdf-xchange.operation_failed':
									ErrorBox.show(msg.errorCode);
									break;
								default:
									console.warn('Unknown msg type: "' + msg.type + '"');
							}
						}
					}
				}

				// Returning "True" here causes error "Uncaught (in promise) Error: A listener indicated an asynchronous response by returning true, but the message channel closed before a response was received"
				// return true;
			}
		);

		$('#cancel-button').on(
			'click',
			function () {
				chrome.runtime.sendMessage({ type: "pdf-x-change.cancel_op", tab_id: gActiveTabID }, function (msg) {
					if (chrome.runtime.lastError) {
						// Could not receive the message
						// console.error(chrome.runtime.lastError.message);
						return;
					}
					$(this).prop('disabled', true);
				});
				return false;
			}
		);

		this.initialized = true;
	},

	show: function (fetch=true) {
		this.init();
		show('.progress-box');
		hide('.actions');
		if (fetch)
			this.fetch();
	},

	fetch: function (showIfAny=false) {
		// Ask for a status in case if one was set while the iframe is loading, to not miss a status.
		chrome.runtime.sendMessage({ type: "pdf-x-change.progress.get", tab_id: gActiveTabID }, (msg) => {
			if (chrome.runtime.lastError) {
				// Could not receive the message
				// the following shows error on extension section on chrome://extensions page
				// console.warn(chrome.runtime.lastError.message);
				return;
			}
			if (msg) {
				if (showIfAny)
					this.show(false);
				this.render(msg);
			}
		});
	},

	render: function (msg) {
		if (msg.type !== "pdf-x-change.progress.post")
			// ignore that message
			return;

		if (!msg.progress && 0 !== msg.progress)
			return;

		if (msg.action && 'string' === typeof msg.action) {
			let action = msg.action.trim();
			if ('...' !== action.substr(-3))
				action += '...';
			$('.progress-action').text(action);
		}

		if (true === msg.can_be_canceled || true === msg.canBeCanceled) {
			$('#cancel-button-box').css('display', 'block').show();
		} else {
			$('#cancel-button-box').css('display', 'none').hide();
		}

		// -1 means intedeterminated progress
		if (-1 === msg.progress) {
			$('#progress-container').find('.progress-bar')
				.addClass('progress-bar-striped progress-bar-animated')
				.css({ width: '100%' });
		}
		else if ('done' === msg.progress) {
			this.hide();
			// hide the entire menu, like Adobe does
			closeMe();
		}
		else {
			$('#progress-container').find('.progress-bar')
				.removeClass('progress-bar-striped progress-bar-animated')
				.css({ width: msg.progress + '%' });
		}
	},

	hide: function () {
		hide('.progress-box');
		show('.actions');
	}
}

const ErrorBox = {
	MESSAGES: {
		1:    chrome.i18n.getMessage('Error_ceErrSaveFile'),
		2:    chrome.i18n.getMessage('Error_ceErrLoad'),
		3:    chrome.i18n.getMessage('Error_ceErrConvert'),
		4:    chrome.i18n.getMessage('Error_ceErrAbort'),
		1024: chrome.i18n.getMessage('Error_ceErrUndefined'),
	},

	show: function (errorCode) {
		Progress.hide();
		$('.error-box p').html('<img src="../images/error.png" /> ' + this.MESSAGES[errorCode]);
		show('.error-box');
	},

	hide: function () {
		hide('.error-box');
		$('.error-box p').html('');
	}
}

function closeMe() {
	window.close();
}

function setPDFView() {
	show('.do_openPDF');
	show('.horizontal-rule');
	show('.api-option-container');
	show('.progress-action');
	hide('.do_convertHTML');
	hide('.do_preferences');

	$(".do_openPDF").val(chrome.i18n.getMessage("OpenPDFCommand")).click(function () {
		ErrorBox.hide();
		chrome.runtime.sendMessage({ operation: "openPDF" })
		Progress.show();
	});

	chrome.storage.sync.get(['showOpenPDFFrame'], function (result) {
		var bCheck = (typeof result.showOpenPDFFrame === "undefined") ? true : result.showOpenPDFFrame;
		$(".always-show").prop("checked", bCheck);
	});

	$(".always-show").click(function () {
		chrome.runtime.sendMessage({
			operation: "showOpenPDFFrame",
			show: $(".always-show").is(":checked")
		});
	});

	$('.progress-action').text(chrome.i18n.getMessage('Canceling') + '...');
	$("#show-open-in-editor-lbl").html(chrome.i18n.getMessage("ShowOpenPDFFrame"));
}

function setHtmlView() {
	hide('.do_openPDF');
	hide('.horizontal-rule');
	hide('.api-option-container');
	show('.do_convertHTML');
	show('.do_preferences');

	$(".do_convertHTML").val(chrome.i18n.getMessage("ConvertToPDFCommand")).click(function (element) {
		if (!gActiveTabID) {
			// Can not proceed here..
			console.error('Trying to start HTML to PDF conversion without tab id being set');
			return;
		}
		ErrorBox.hide();
		chrome.runtime.sendMessage({ operation: "convertHTMLToPDF", tab_id: gActiveTabID });
		Progress.show();
	});
	$(".do_preferences").val(chrome.i18n.getMessage("PreferencesCommand")).click(function (element) {
		chrome.runtime.sendMessage({ operation: "preferences" });
		closeMe();
	});
}

function setView() {
	Progress.fetch(true);

	switch (gActiveDocumentType) {
		case 'application/pdf':
			$('.loading-box').remove();
			setPDFView();
			break;

		case 'text/html':
			$('.loading-box').remove();
			setHtmlView();
			break;

		default:
			$('.loading-box').html('Unknown document type');
			console.warn('Unknown document type "' + doctype + '"');
	}
}

$(function () {

	chrome.tabs.query({
		currentWindow: true,
		active: true
	}, (tabs) => {
		if (0 === tabs.length) {
			console.error('Can not fetch active tab');
			$('.loading-box').remove();
			return;
		}
		const tabID = tabs[0].id;
		gActiveTabID = tabID;
		chrome.scripting.executeScript(
			{
				target: { tabId: tabID },
				func: () => {
					return {
						doctype: document.contentType
					};
				}
			},
			(injectionResults) => {
				if (!injectionResults) {
					console.warn('injectionResults is empty with tabID', injectionResults, tabID);
					// nothing, really, to do
					$('.loading-box').remove();
					return;
				}
				// I do not expect more than 1 result here...
				if (injectionResults.length !== 1) {
					console.warn('Got unexpected number of results in popup script, when reading document type')
				}
				let doctype = null;
				if (injectionResults[0] && injectionResults[0].result && injectionResults[0].result.doctype)
					doctype = injectionResults[0].result.doctype;
				if (!doctype) {
					$('.loading-box').html('<p style="color: red">Can not define content type of the currently opened document</p>')
					return;
				}
				gActiveDocumentType = doctype;
				setView();
			}
		);
	});

	$('.close-dialog').click(function () {
		closeMe();
	});

	chrome.runtime.onMessage.addListener(
		function (msg, sender, sendResponse) {
			if (msg.operation && msg.operation === 'documentLoaded') {
				if (!gActiveTabID) {
					console.warn('Document has loaded, but we do not know the active tab in popup/index.js');
					return;
				}
				if (sender.tab.id == gActiveTabID) {
					// lets look at doctype
					if (gActiveDocumentType) {
						if (msg.doctype != gActiveDocumentType) {
							console.warn('doctype came from "documentLoaded event differs from the already set gActiveDocumentType')
						}
					} else {
						gActiveDocumentType = msg.doctype;
						setView();
					}
				}
			}
		}
	);

})