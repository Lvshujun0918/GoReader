function initialize()
{
	let g_PDFDOcumentIsOpening = false;
	const $actionsBox = $(".actions .click-area");
	$actionsBox.find('.label').text(chrome.i18n.getMessage("OpenPDFCommand"));
	$actionsBox.click(function (element) {
		if (WidgetMover.wasMoved) {
			WidgetMover.wasMoved = false;
		} else {
			if (!g_PDFDOcumentIsOpening) {
				g_PDFDOcumentIsOpening = true;
				chrome.runtime.sendMessage({ operation: "openPDF" })
			}
		}
	});

	$(".close-dialog").click(function(event) {
		event.stopPropagation();
		event.preventDefault();
		chrome.runtime.sendMessage({ operation: "showOpenPDFFrame", show: false })
	});

	// Listening for progress message to draw a progress bar..
	chrome.runtime.onMessage.addListener(
		function (msg, sender, sendResponse) {
			if (msg.type !== "pdf-x-change.progress.post")
				// ignore that message
				return;
			if (!msg.progress && 0 !== msg.progress)
				return;
			if (-1 === msg.progress) {
				$('#progress-container').hide();
				$('#indeterminated-progress-container').show();
			}
			else if ('done' === msg.progress) {
				$('#indeterminated-progress-container').hide();
				$('#progress-container').hide();
				g_PDFDOcumentIsOpening = false;
			}
			else {
				$('#indeterminated-progress-container').hide();
				$('#progress-container').show();
				$('#progress-container').find('.progress-bar').css({ width: msg.progress + '%' });
			}
		}
	);
}

WidgetMover = {

	isInitialized: false,

	active: false,

	wasMoved: false,

	initialize: function () {
		if (this.isInitialized)
			return;
		const mr = this;
		$(document).on('mousemove', function (e) {
			if (mr.active) {
				const originalEvent = e.originalEvent;
				const { movementX, movementY, clientX, clientY } = originalEvent;
				window.parent.postMessage({
					action: 'pdf-xchange.open_PDF_IFrame.move',
					moveData: { movementX, movementY, clientX, clientY }
				}, '*');
				if (movementX !== 0 || movementY !== 0)
					mr.wasMoved = true;
			}
			return false;
		});
		this.isInitialized = true;
	},

	start: function () {
		this.initialize();
		this.active = true;
	},

	stop: function () {
		window.parent.postMessage({
			action: 'pdf-xchange.open_PDF_IFrame.set_offset',
		}, '*');
		this.active = false;
	}
}

jQuery(function() {
	initialize();
	$(document).on('mousedown', function () {
		WidgetMover.start();
	}).on('mouseup', function () {
		WidgetMover.stop();
	}).on('mouseleave', function () {
		WidgetMover.stop();
	});
});