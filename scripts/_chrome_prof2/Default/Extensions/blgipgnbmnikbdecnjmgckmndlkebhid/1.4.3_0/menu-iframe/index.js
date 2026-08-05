const DEBUG = true;
function print_debug(msg, ...args) {
	if (DEBUG) {
		const now = new Date();
		console.debug(`${now.getUTCHours()}:${now.getUTCMinutes()}:${now.getUTCSeconds()} [PDF-XCHANGE DEBUG]: ${msg}`, ...args);
	}
}
const show = (selector) => $(selector).removeClass('hidden');
const hide = (selector) => $(selector).addClass('hidden');

function sendChromeRuntimeMessage(args, onAfterMessageIsSent)
{
	try {
		chrome.runtime.sendMessage(args);
		if ($.isFunction(onAfterMessageIsSent))
			onAfterMessageIsSent();
	} catch (e) {
		if (e.message.startsWith('Extension context invalidated')) {
			// Just silently hide the menu
			closeMe();
		} else {
			throw e;
		}
	}
}

const Progress = {

	initialized: false,

	init(params) {
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
					switch (msg.type) {
						case 'pdf-x-change.progress.post':
							Progress.render(msg);
							break;
						case 'pdf-xchange.operation_failed':
							ErrorBox.show(msg);
							break;
					}
				}

				// Uncaught (in promise) Error: A listener indicated an asynchronous response by returning true, but the message channel closed before a response was received
				// return true;
			}
		);

		$('#cancel-button').on(
			'click',
			function () {
				const $btn = $(this);
				try {
					const p = chrome.runtime.sendMessage({ type: "pdf-x-change.cancel_op", tab_id: params.tab_id }, function (msg) {
						if (chrome.runtime.lastError) {
							// Could not receive the message
							// console.error(chrome.runtime.lastError.message);
							return;
						}
						$btn.prop('disabled', true);
					});
				} catch (ex) {
					if (ex.message.startsWith('Extension context invalidated')) {
						// Just silently hide the menu
						closeMe();
					} else {
						throw ex;
					}
				}

				// Documentation says chrome.runtime.sendMessage returns promise, but p is actually undefined here..
				// p.catch((err) => {
				//    console.warn('chrome.runtime.sendMessage() :: promise catch() called', err);
				//    closeMe();
				// });

				return false;
			}
		);

		this.initialized = true;
	},

	show: function (params, fetch=true) {
		this.init(params);
		show('.progress-box');
		hide('.actions');
		if (fetch)
			this.fetch(params);
	},

	fetch: function (params, showIfAny=false) {
		// Ask for a status in case if one was set while the iframe is loading, to not miss a status.
		chrome.runtime.sendMessage({ type: "pdf-x-change.progress.get", tab_id: params.tab_id }, (msg) => {
			if (chrome.runtime.lastError) {
				// Could not receive the message
				// the following shows error on extension section on chrome://extensions page
				console.warn(chrome.runtime.lastError.message);
				return;
			}
			if (msg) {
				print_debug(`Progress.fetch() received msg`, msg);
				if (showIfAny)
					this.show(params, false);
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

		if (true === msg.can_be_canceled) {
			$('#cancel-button-box').show();
		} else {
			$('#cancel-button-box').hide();
		}

		// -1 means intedeterminated progress
		if (-1 === msg.progress) {
			$('#progress-container').find('.progress-bar')
				.removeClass('with-transition')
				.addClass('progress-bar-striped progress-bar-animated')
				.css({ width: '100%' });
			$('#progress-value-in-percents').css('visibility', 'hidden').text('');
			window.parent.postMessage({ action: 'pdf-xchange.menu-iframe.cancel_fading_me_out' }, '*');
		}
		else if ('done' === msg.progress) {
			this.hide();
			if (!msg.finish_state || msg.finish_state == 2) { // do not hide if some error crops up
				// hide the entire menu, like Adobe does
				window.parent.postMessage({ action: 'pdf-xchange.menu-iframe.fade_me_out' }, '*');
			}
		}
		else {
			// update determinated progress
			const $progressBar = $('#progress-container').find('.progress-bar');
			$progressBar
				.removeClass('progress-bar-striped progress-bar-animated')
				.css({ width: msg.progress + '%' })
				;
			setTimeout(() => {
				if (!$progressBar.hasClass('with-transition'))
					$progressBar.addClass('with-transition')
			}, 10);
			$('#progress-value-in-percents').css('visibility', 'visible').text(`${msg.progress} %`);
			window.parent.postMessage({ action: 'pdf-xchange.menu-iframe.cancel_fading_me_out' }, '*');
		}
	},

	hide: function () {
		hide('.progress-box');
		show('.actions');
	}
}

const ERROR_CANCELLED = 1223;
function DS_GET_ECODE(x) {
	return x & 0x0ffff;
}
const ErrorBox = {
	MESSAGES: {
		1: chrome.i18n.getMessage('Error_ceErrSaveFile'),
		2: chrome.i18n.getMessage('Error_ceErrLoad'),
		3: chrome.i18n.getMessage('Error_ceErrConvert'),
		4: chrome.i18n.getMessage('Error_ceErrAbort'),
		1024: chrome.i18n.getMessage('Error_ceErrUndefined'),
	},

	show: function (msg) {
		print_debug('Message came into ErrorBox.show() :', msg);
		if ('undefined' !== typeof msg.state) {
			if (3 == msg.api_version) {
				const ds_ecode = DS_GET_ECODE(msg.state);
				if (msg && ERROR_CANCELLED == ds_ecode) {
					$('.error-box p').html(`<img src="../images/error.png" /> `
						+ `<span class="state-part">${chrome.i18n.getMessage('Op_Canceled')}</span><br />`
					);
					show('.error-box');
				} 
				else if (ds_ecode) {
					$('.error-box p').html(`<img src="../images/error.png" /> `
						+ `<span class="state-part">${chrome.i18n.getMessage('Error_ceErrFailed')}; `
							+`<span style="white-space: nowrap">${chrome.i18n.getMessage('Error_ceErrCode')}: 0x${(msg.state >>> 0).toString(16)}</span>`
							+`</span><br />`
					);
					show('.error-box');
				} else {
					console.error('Empty error code');
				}
			}
			else {
				if (1 == msg.state) {
					let message = 'undefined' == typeof msg.message ? '' : msg.message;
					$('.error-box p').html(`<img src="../images/error.png" /> `
						+ `<span class="state-part">${chrome.i18n.getMessage('Error_ceErrConvert')}</span><br />`
						+ `<span class="message-part">${message}</span>`
					);
					show('.error-box');
				} 
				else if (2 == msg.state) {
					$('.error-box p').html(`<img src="../images/error.png" /> `
						+ `<span class="state-part">${chrome.i18n.getMessage('Op_Canceled')}</span><br />`
						+ `<span class="message-part"></span>`
					);
					show('.error-box');
				} 
				else {
					console.error('Unknown "state" value in ErrorBox.show() : ', msg.state);
				}
			}
		} else if (msg.message) {
			// Update just a message
			// It can come later in another message from Native Host in version 2 protocol
			if ($('.error-box p .state-part').length > 0) {
				$('.error-box p .message-part').text(msg.message)
			} else {
				console.warn('Message has come without state, while state element is not in the DOM');
				$('.error-box p').html(`<img src="../images/error.png" /> `
					+ `<span class="state-part"></span><br />`
					+ `<span class="message-part">${msg.message}</span>`
				);
			}
			show('.error-box');
		} else if (msg.errorCode) {
			// Native Host Protocol Version 1
			Progress.hide();
			$('.error-box p').html('<img src="../images/error.png" /> ' + this.MESSAGES[msg.errorCode]);
			show('.error-box');
		}
		
	},

	hide: function () {
		const $errorBox = $('.error-box');
		let errorHeight = 0;
		if ($errorBox.is(':visible'))
			errorHeight = $errorBox.outerHeight();
		hide('.error-box');
		$('.error-box p').html('');
		if (errorHeight > 0) {
			window.parent.postMessage({
				action: 'pdf-xchange.menu-iframe.lower_height',
				px: errorHeight
			}, '*');
		}
	}
}

function closeMe() {
	window.parent.postMessage({ action: 'pdf-xchange.menu-iframe.close_me' }, '*');
}

$(function () {
	const queryParameters = Object.fromEntries((new URLSearchParams(window.location.search)).entries());
	let { p } = queryParameters;
	p = JSON.parse(p);
	const initWithProgress = 'undefined' === typeof p.init_with_progress || true === p.init_with_progress;

	if (initWithProgress)
		Progress.fetch(p, true);

	print_debug(`iFrame initialized with params`, p);	
	
	switch (p.doctype) {
		case 'application/pdf':
			$('.loading-box').remove();
			show('.do_openPDF');
			show('.horizontal-rule');
			show('.api-option-container');
			show('.progress-action');
			hide('.do_convertHTML');
			hide('.do_preferences');

			$(".do_openPDF").val(chrome.i18n.getMessage("OpenPDFCommand")).click(function () {
				ErrorBox.hide();
				sendChromeRuntimeMessage({ operation: "openPDF" }, () => {
					Progress.show(p);
				});
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

			// $('.progress-action').text(chrome.i18n.getMessage('Canceling') + '...');
			$("#show-open-in-editor-lbl").html(chrome.i18n.getMessage("ShowOpenPDFFrame"));

			break;

		case 'text/html':
			$('.loading-box').remove();
			hide('.do_openPDF');
			hide('.horizontal-rule');
			hide('.api-option-container');
			show('.do_convertHTML');
			show('.do_preferences');

			$(".do_convertHTML").val(chrome.i18n.getMessage("ConvertToPDFCommand")).click(function (element) {
				ErrorBox.hide();
				sendChromeRuntimeMessage({ operation: "convertHTMLToPDF" }, () => {
					Progress.show(p);
				});
			});
			$(".do_preferences").val(chrome.i18n.getMessage("PreferencesCommand")).click(function (element) {
				chrome.runtime.sendMessage({ operation: "preferences" });
				closeMe();
			});

			break;

		default:
			$('.loading-box').html('Unknown document type');
			console.warn('Unknown document type "' + p.doctype + '"');
	}
	

	$('.close-dialog').click(function () {
		closeMe();
	});
})