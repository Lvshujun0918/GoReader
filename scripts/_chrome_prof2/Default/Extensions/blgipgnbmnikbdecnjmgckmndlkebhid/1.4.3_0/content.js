// Incapsulates all work with extension popup, implemented as iFrame, injected into the loaded web page
const MenuIframe =
{
	$instance: null,
	fadingOut: false,
	fadingOutTimeout: null,

	inject: function (p) {
		if (0 === (this.$instance = $("#__pdfxcnangeMenuDialog__")).length) {
			this.$instance = $("<iframe>")
				.attr("id", "__pdfxcnangeMenuDialog__")
				.css({
					'border': '0px',
					'zIndex': 2147483647,
					'position': 'fixed',
					'top': '-1px',
					'right': '80px',
					'width': '294px',
					'height': '165px',
					'display': 'block',
					'margin': 'auto',
					'boxShadow': '0 1px 4px #00000026'
				})
				.attr("src", chrome.runtime.getURL("menu-iframe/index.html?p=" + encodeURIComponent(JSON.stringify({
					doctype: p.doctype,
					tab_id: p.tab_id,
					init_with_progress: p.init_with_progress
				}))))
				.appendTo("html");

			// when an error, just make the iframe a bit more height
			chrome.runtime.onMessage.addListener(
				function (msg, sender, sendResponse) {
					if (chrome.runtime.lastError) {
						// Could not receive the message
						console.error(chrome.runtime.lastError.message);
						return;
					}

					if (msg && 'pdf-xchange.operation_failed' === msg.type) {
						if (MenuIframe.$instance)
							MenuIframe.$instance.css('height', '210px');
					}

					// Uncaught (in promise) Error: A listener indicated an asynchronous response by returning true, but the message channel closed before a response was received
					// return true;
				}
			);
		}
	},

	dismantle: function () {
		if (this.$instance)
			this.$instance.remove();
		this.$instance = null;
	},

	hide: function () {
		if (this.$instance)
			this.$instance.hide();
	},

	show: function () {
		if (this.$instance)
			this.$instance.show();
	},

	fadeOut: function () {
		if (this.fadingOut || !this.$instance)
			return;
		this.fadingOut = true;
		const _this = this;
		this.$instance.fadeOut('slow', function () {
			_this.dismantle();
			this.fadingOut = false;
		});
	},

	fadeOutDelayed: function (delay) {
		this.fadingOutTimeout = setTimeout(() => this.fadeOut(), delay);
	},

	cancelFadingOut: function () {
		if (this.fadingOutTimeout) {
			clearTimeout(this.fadingOutTimeout);
			this.fadingOutTimeout = null;
		}
	},

	toggle: function (injectP) {
		(this.$instance && this.$instance.length)
			? this.dismantle()
			: (this.inject(injectP), this.show(injectP));
	},

	lowerHeight: function (px) {
		if (this.$instance)
			this.$instance.css('height', (this.$instance.height() - px) + 'px');
	}
}

const NoNativePartAlert = {
	$instance: null,
	inject: function () {
		if (0 === (this.$instance = $("#__pdfxcnangeNoNativePartAlert__")).length) {
			this.$instance = $("<iframe>")
				.attr("id", "__pdfxcnangeNoNativePartAlert__")
				.css({
					'border': '1px solid black',
					'background': '#fafafa 0 0 no- repeat padding - box',
					'zIndex': 2147483647,
					'position': 'fixed',
					'top': '20%',
					'left': '45%',
					'width': '400px',
					'height': '195px',
					'display': 'block',
					'margin': 'auto',
					'boxShadow': '0 1px 4px #00000026',
					'borderRadius': '4px'
				})
				.attr("src", chrome.runtime.getURL("native-part-is-not-installed.html"))
				.appendTo("html");
		}
	},
	dismantle: function () {
		if (this.$instance)
			this.$instance.remove();
		this.$instance = null;
	},
}

let $newiframe;

const WIDGET_WIDTH = 275;
const PURE_WIDGET_HEIGHT = 50;
let WIDGET_HEIGHT = PURE_WIDGET_HEIGHT;
const WIDGET_WITH_PROGRESS_HEIGHT = 63;

function injectOpenPDFIFrame(offset) {
	if (!offset || !offset.top || !offset.left && !offset.right)
		offset = { top: 56, right: 12 };
	const sURL = chrome.runtime.getURL("frame.html");
	if (0 === ($newiframe = $("#__pdfxcnangeNewDialog__")).length) {
		const css = {
			border: "0px",
			position: "fixed",
			top: offset.top + "px",
			right: offset.right + "px",
			width: WIDGET_WIDTH + "px",
			height: WIDGET_HEIGHT + "px",
			display: "block",
			zIndex: 10,

			// The following settings mess up the btn dragging..
			// margin: "auto"
		}

		if (offset.left)
			css.left = offset.left + "px";
		else if (offset.right)
			css.right = offset.right + "px";

		$newiframe = $("<iframe>")
			.attr("id", "__pdfxcnangeNewDialog__")
			.css(css)
			.attr("src", sURL)
			.appendTo("html");

		// react on the progress/hide show to adjust iframe height
		// Listening for progress message to draw a progress bar..
		chrome.runtime.onMessage.addListener(
			function (msg, sender, sendResponse) {
				if (msg.type !== "pdf-x-change.progress.post")
					// ignore that message
					return;
				if (!msg.progress && 0 !== msg.progress)
					return;
				if (-1 === msg.progress) {
					WIDGET_HEIGHT = WIDGET_WITH_PROGRESS_HEIGHT;
					$newiframe.css('height', WIDGET_HEIGHT + 'px');
				}
				else if ('done' === msg.progress) {
					WIDGET_HEIGHT = PURE_WIDGET_HEIGHT;
					$newiframe.css('height', WIDGET_HEIGHT + 'px');
				}
				else {
					WIDGET_HEIGHT = WIDGET_WITH_PROGRESS_HEIGHT;
					$newiframe.css('height', WIDGET_HEIGHT + 'px');
				}
			}
		);
	}
}

function removeOpenPDFIFrame() {
	if ($newiframe)
		$newiframe.remove();
}

$(document).ready(function () {
	chrome.runtime.onMessage.addListener(function (e) {
		switch (e.content_op) {
			case 'showOpenPDFIFrame':
				if (e.show) {
					chrome.storage.sync.get(['OpenPDFFrameOffset'], function (result) {
						injectOpenPDFIFrame((typeof result.OpenPDFFrameOffset === "undefined") ? null : result.OpenPDFFrameOffset);
					});
				} else {
					removeOpenPDFIFrame();
				}
				break;
			case 'showBrowserActionPseudoPopup':
				MenuIframe.toggle({
					doctype: e.doctype,
					tab_id: e.tab_id,
					init_with_progress: e.init_with_progress
				});
				break;
			case 'pdf-x-change.native_part_is_not_installed':
				NoNativePartAlert.inject();
				break;
			case 'pdf-x-change.native_part_is_not_permitted_by_admin':
				alert(e.message);
				break;
			case 'pdf-xchange.menu-iframe.close_me':
				MenuIframe.dismantle();
				break;
			case 'pdf-xchange.menu-iframe.hide_me':
				MenuIframe.hide();
				break;
			case 'pdf-xchange.menu-iframe.show_me':
				MenuIframe.show();
				break;
		}
	});

	window.addEventListener("message", (event) => {
		if (event.source && event.source.parent === window && event.data) {
			switch (event.data.action) {
				case 'pdf-xchange.menu-iframe.fade_me_out':
					MenuIframe.fadeOutDelayed(2500);
					break;
				case 'pdf-xchange.menu-iframe.cancel_fading_me_out':
					MenuIframe.cancelFadingOut();
					break;
				case 'pdf-xchange.menu-iframe.close_me':
					MenuIframe.dismantle();
					break;
				case 'pdf-xchange.no-native-iframe.close_me':
					NoNativePartAlert.dismantle();
					break;
				case 'pdf-xchange.menu-iframe.lower_height':
					MenuIframe.lowerHeight(event.data.px);
					break;
				case 'pdf-xchange.open_PDF_IFrame.move':
					OpenPDFWidgetMover.move(event.data.moveData);
					break;
				case 'pdf-xchange.open_PDF_IFrame.set_offset':
					chrome.storage.sync.set({ OpenPDFFrameOffset: OpenPDFWidgetMover.offset });
					break;
			}
		}

		// We only accept messages from ourselves
		if (event.source != window)
			return;

		if (event.data.type && (event.data.type == "PDFXCHANGE_ADDIN_DOC_URL")) {
			console.log("Content script received: " + event.data.url);
			try
			{
				chrome.runtime.sendMessage({ operation: "sharepoint_addin_doc_url", url: event.data.url });
			} catch (err) {
				console.error('Unable to sendMessage sharepoint_addin_doc_url');
				console.error(err);
			}
		}

		if (event.data.type && (event.data.type == "PDFXCHANGE_GET_VERSION")) {
			console.log("Content script received - version request");
			//			console.log("Tab id:" + chrome.tabs);
			window.postMessage({ type: "PDFXCHANGE_VERSION", version: "1.4.3" }, "*");
		}
	}, false);

	window.postMessage({ type: "PDFX_EXTENSION_LISTENING" }, "*");
});

$(window).on('load', function () {
	chrome.runtime.sendMessage({
		operation: "documentLoaded",
		doctype: document.contentType
	});
});

OpenPDFWidgetMover = {

	isMouseCursorOutOfPage: false,
	offset: {},

	move: function (moveData) {
		const offset = $newiframe.offset();

		let isMouseCursorOutOfPage = false;
		if (offset.top + moveData.clientY < 0) {
			isMouseCursorOutOfPage = 'top';
		} else if (offset.left + moveData.clientX < 0) {
			isMouseCursorOutOfPage = 'left';
		} else if (offset.top + moveData.clientY > window.innerHeight) {
			isMouseCursorOutOfPage = 'bottom';
		} else if (offset.left + moveData.clientX > window.innerWidth) {
			isMouseCursorOutOfPage = 'right';
		}
		switch (isMouseCursorOutOfPage) {
			case 'top':
				if (!this.isMouseCursorOutOfPage) {
					offset.top = 0;
				}
				this.isMouseCursorOutOfPage = isMouseCursorOutOfPage;
				break;
			case 'left':
				if (!this.isMouseCursorOutOfPage) {
					offset.left = 0;
				}
				this.isMouseCursorOutOfPage = isMouseCursorOutOfPage;
				break;
			case 'bottom':
				if (!this.isMouseCursorOutOfPage) {
					offset.top = window.innerHeight - WIDGET_HEIGHT;
				}
				this.isMouseCursorOutOfPage = isMouseCursorOutOfPage;
				break;
			case 'right':
				if (!this.isMouseCursorOutOfPage) {
					offset.left = window.innerWidth - WIDGET_WIDTH;
				}
				this.isMouseCursorOutOfPage = isMouseCursorOutOfPage;
				break;
			default:
				switch (this.isMouseCursorOutOfPage) {
					case 'top':
						// The cursor is back to page from top
						this.isMouseCursorOutOfPage = false;
						offset.top += moveData.movementY;
						if (offset.top > 0)
							offset.top = 0;
						if (moveData.clientX < 0) {
							offset.left += moveData.clientX;
						} else if (moveData.clientX > WIDGET_WIDTH) {
							offset.left += moveData.clientX - WIDGET_WIDTH + 10;
						}
						break;

					case 'left':
						this.isMouseCursorOutOfPage = false;
						offset.left += moveData.movementX;
						if (offset.left > 0)
							offset.left = 0;
						if (moveData.clientY < 0) {
							offset.top += moveData.clientY;
						} else if (moveData.clientY > WIDGET_HEIGHT) {
							offset.top += moveData.clientY - WIDGET_HEIGHT + 25;
						}
						break;

					case 'right':
						this.isMouseCursorOutOfPage = false;
						offset.left += moveData.movementX;
						if (moveData.clientX > WIDGET_WIDTH)
							offset.left = window.innerWidth - WIDGET_WIDTH + 15;
						if (moveData.clientY < 0) {
							offset.top += moveData.clientY;
						} else if (moveData.clientY > WIDGET_HEIGHT) {
							offset.top += moveData.clientY - WIDGET_HEIGHT + 25;
						}
						break;

					case 'bottom':
						// The cursor is back to page from the bottom
						this.isMouseCursorOutOfPage = false;
						offset.top += moveData.movementY;
						if (moveData.clientY > WIDGET_HEIGHT)
							offset.top = window.innerHeight - WIDGET_HEIGHT + 35;
						if (moveData.clientX < 0) {
							offset.left += moveData.clientX;
						} else if (moveData.clientX > WIDGET_WIDTH) {
							offset.left += moveData.clientX - WIDGET_WIDTH + 10;
						}
						break;

					default:
						offset.top += moveData.movementY;

						if (offset.top < 0)
							offset.top = 0;
						if (offset.top > window.innerHeight - WIDGET_HEIGHT)
							offset.top = window.innerHeight - WIDGET_HEIGHT


						offset.left += moveData.movementX;

						if (offset.left < 0)
							offset.left = 0;
						if (offset.left > window.innerWidth - WIDGET_WIDTH)
							offset.left = window.innerWidth - WIDGET_WIDTH
				}
		}

		this.offset = offset;
		$newiframe.offset(offset);
	}
}