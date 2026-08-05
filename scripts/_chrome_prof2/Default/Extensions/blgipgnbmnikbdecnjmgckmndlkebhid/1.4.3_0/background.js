const NativeHostName = 'com.trackersoftware.htmltopdf';
let NativeHostVersion = 2; // let's think we are working with the newest by default
const NativeHostsPorts = {};

const POPUP_TYPE_IFRAME = 'iframe'
const POPUP_TYPE_POPUP  = 'popup'
const POPUP_TYPE = POPUP_TYPE_IFRAME; // "iframe" or "popup" is possible

function getTabInfoStorageKey(tabID) {
	return 'pdf-xchange-tab-info_' + tabID + '_tab';
}

const DEBUG = true;
function print_debug(msg, ...args) {
	if (DEBUG) {
		const now = new Date();
		console.debug(`${now.getUTCHours()}:${now.getUTCMinutes()}:${now.getUTCSeconds()} [PDF-XCHANGE DEBUG]: ${msg}`, ...args);
	}
}

function DS_GET_ECODE(x) {
	return x & 0x0ffff;
}

const ERROR_CANCELLED = 1223;
class NativeMessage {
	rawMessage = {};
	constructor(rawMessage) {
		this.rawMessage = rawMessage;
	}

	getVersion() {
		return typeof this.rawMessage.version !== 'undefined' ? parseInt(this.rawMessage.version) : 1;
	}

	static process(tabID, instance) {
		print_debug(`Received message: ${JSON.stringify(instance.rawMessage)}`);
		const NHMsg = instance.rawMessage;
		if (NHMsg.action == "responceDone" || 'responseDone' == NHMsg.action) {
			switch (instance.getVersion()) {
				case 1:
					{
						ProgressesManager.stopProgressForTab(tabID, NHMsg.error ? 1 : 0);
						if (NHMsg.error)
							reportOperationFailure(tabID, { errorCode: NHMsg.error });
					}
					break;
				case 2:
					{
						ProgressesManager.stopProgressForTab(tabID, NHMsg.state);
						switch (NHMsg.state) {
							case 0: // Success
								break;
							case 1: // Fail
								reportOperationFailure(tabID, { state: NHMsg.state });
								break;
							case 2: // Canceled
								reportOperationFailure(tabID, { state: NHMsg.state });
								break;
							default:
								console.error('Unknown "state" value came with responseDone action');
						}
					}
					break;
				case 3:
					{
						const error = DS_GET_ECODE(NHMsg.state);
						if (error === ERROR_CANCELLED)
						{
							ProgressesManager.stopProgressForTab(tabID, 2);

							// Show Cancelled message here
							reportOperationFailure(tabID, { 
								state: NHMsg.state,
								api_version: 3,
							});
						}
						else if (error !== 0)
						{
							ProgressesManager.stopProgressForTab(tabID, NHMsg.state);

							// Show Error message here
							reportOperationFailure(tabID, {
								state: NHMsg.state, 
								api_version: 3,
							});
						}
						else
						{
							ProgressesManager.stopProgressForTab(tabID, 0);
						}
					}
					break;
				default:
					console.error('Unknown responceDone action version');
			}
		}
		else if (NHMsg.action == "HTML2PDFProgress") {
			switch (instance.getVersion()) {
				case 1:
					{
						let progressValue = instance.rawMessage.value;
						if (progressValue || 0 === progressValue || '0' === progressValue) {
							progressValue = parseInt(progressValue);
							if (progressValue >= 0 && progressValue <= 100) {
								ProgressesManager.getProgressForTab(tabID).then((progress) => {
									progress.setProgress('HTMLToPDFConverting', progressValue, true);
								});
							} else {
								console.error(`Native Handler returns unrecognized progress value for action "${NHMsg.action}". Should be unsigned int between 0 and 100`);
							}
						} else {
							console.error('Native Handler returns empty progress value for action ', NHMsg.action, instance.rawMessage.value);
						}
					}
					break;
				default:
					console.error('Unknown HTML2PDFProgress action version');
			}
		}
		else if (NHMsg.action == "sendNextData") {
			switch (instance.getVersion()) {
				case 1:
					DataTransmitter.sendNextChunk(tabID);
					break;
				default:
					console.error('Unknown sendNextData action version');

			}
		} else if (NHMsg.action == "HTML2PDFError") {
			switch (instance.getVersion()) {
				case 1:
					{
						if (NHMsg.value) {
							reportOperationFailure(tabID, { state: 1, message: NHMsg.value });
						} else {
							console.error("\"value\" field should be set and should not be empty for \"HTML2PDFError\" action");
						}
					}
					break;
				default:
					console.error('Unknown HTML2PDFError action version');
			}
		} else {
			console.warn("Unknown action has been received from Native Handler: ", instance.rawMessage);
		}
	}
}

function onNativeMessage(tabID, message) {
	NativeHostVersion = message.version || 1;
	NativeMessage.process(tabID, new NativeMessage(message));
}

function connect(tabID) {
	const port = chrome.runtime.connectNative(NativeHostName);
	NativeHostsPorts[tabID] = port;
	port.onMessage.addListener((function (tabID) {
		return function (message) {
			onNativeMessage(tabID, message);
		}
	})(tabID));
	port.onDisconnect.addListener((function (tabID) {
		return function () {
			const rt = chrome.runtime;
			const leMsg = rt.lastError.message;
			print_debug(`Native Handler has disconnected (tabID: ${tabID}): ${leMsg}`);
			if (leMsg.includes("not found") || leMsg.includes("forbidden"))
				chrome.tabs.sendMessage(tabID, { content_op: "pdf-x-change.native_part_is_not_installed" });
			if (leMsg.includes("disabled by the system administrator"))
				chrome.tabs.sendMessage(tabID, {
					content_op: "pdf-x-change.native_part_is_not_permitted_by_admin",
					message: leMsg
				});
			ProgressesManager.stopProgressForTab(tabID);
			delete NativeHostsPorts[tabID];
		}
	})(tabID));
	return port;
}

function sendNativeMessage(tabID, message) {
	try {
		// print_debug(`Message to send to Native Host from tab ${tabID}: `, message);
		(NativeHostsPorts[tabID] || connect(tabID)).postMessage(message);
	} catch (err) {
		// Some weird stuff is going on...
		console.error(err);
		reportOperationFailure(tabID, { errorCode: 1024 });
	}
}

function cancelNativeAction(tabID) {
	if (tabID) {
		sendNativeMessage(tabID, { "action": "cancel", "version": 1.0 });
	}
	else {
		console.error('The action to be cancelled has to be tied with a browser tab!')
	}
}

async function doHTMLToPDF(message, sender, sendResponse) {
	// fetch active tab, based on the popup type
	let tab;
	if (POPUP_TYPE === POPUP_TYPE_IFRAME) {
		let tabs = await chrome.tabs.query({ currentWindow: true, active: true });
		if (tabs.length > 0) {
			tab = tabs[0];
		} else {
			console.error(`No active tab is fetched once is "Convert to HTML" action is triggered from popup ${POPUP_TYPE}`);
			return;
		}
	} else if (POPUP_TYPE === POPUP_TYPE_POPUP) {
		if (message.tab_id) {
			tab = await chrome.tabs.get(message.tab_id);
			if (!tab) {
				console.error(`No active tab is fetched once is "Convert to HTML" action is triggered from popup ${POPUP_TYPE}`);
				return;
			}
		} else {
			console.error(`tab_id should be provided for "Convert to HTML" action when triggered from popup ${POPUP_TYPE}`);
		}
	} else {
		console.error('Unknown popup type when is "Convert to HTML" action is triggered');
		return;
	}

	const tabID = tab.id;
	const progress = await ProgressesManager.getProgressForTab(tabID);
	progress.setProgress('HTMLToPDFConverting', -1, true);
		
	const tabTitle = tab.title;
	const tabURL = tab.url;

	chrome.pageCapture.saveAsMHTML({ tabId: tabID }, (mhtmlData) => {
		// @todo: chrome.pageCapture.saveAsMHTML might potentially take a signifiant time, but wont be reflected in progress...

		// We can not cancel the progress here, as the "Save file" dialog is given to user...
		// UPDATE: No yet. in the new NativeHostHandler revision, that dialog is shown at the end of conversion process
		// progress.setCanBeCanceled(false);

		const reader = new FileReader();
		reader.addEventListener('loadend', (event) => {
			// strip out our auxiliary iframe..
			const a_gap = '[=\r\n]*?'
			const iFrameID = '__pdfxcnangeMenuDialog__';
			let regexp = '<' + a_gap + 'i' + a_gap + 'f' + a_gap + 'r' + a_gap + 'a' + a_gap + 'm' + a_gap +'e[^<>]+?id(?:=|='+a_gap+'=)3D"' + a_gap;
			for (let j = 0; j < iFrameID.length; j++)
				regexp += iFrameID[j] + a_gap;
			regexp += '"[^<>]+?>[^]*?<' + a_gap + '\/' + a_gap + 'i' + a_gap + 'f' + a_gap + 'r' + a_gap + 'a' + a_gap + 'm' + a_gap + 'e' + a_gap +'>';
			const mhtml = event.target.result.replace(new RegExp(regexp, 'm'), '');
			DataTransmitter.sendData(tabID, mhtml, tabTitle, tabURL, "MHTMLoPDF", 2.0);
		});
		reader.readAsText(mhtmlData);
	});

	// @todo: replace the chrome.tabs.executeScript() with chrome.scription.executeScript() in the following code if this is ever going to be used...
	//	console.log("OnHTMLToPDFTabs 0");

	//	var e = "var maxSize = " + 1e3 + ", DEBUG = " + !1 + ", TABID = " + tabs[0].id + ", OP = 'html-blob', EXCLUDE = [];";

	//	chrome.tabs.executeScript(tabs[0].id,
	//		{code: e, allFrames: !0},
	//		function () {
	//			chrome.tabs.executeScript(tabs[0].id, {file: "get-html.js", allFrames: !0})
	//		});

	//	chrome.tabs.executeScript(tabs[0].id, {file: "get_webpage.js"})

	//	console.log("OnHTMLToPDFTabs 1");

}

function doPreferences(message, sender, sendResponse) {
	chrome.tabs.query({ currentWindow: true, active: true }, (tabs) => {
		const tabID = tabs[0].id;
		sendNativeMessage(tabID, { "action": "preferences", "version": 1.0 });
	});
}

// A buffer to split message, sent to NativeHost by chunks
function TabBuffer(tabID, data, tabTitle, tabURL, action, version) {
	this.tabID = tabID;
	this.data = data;
	this.sentBytes = 0;
	this.tabTitle = tabTitle;
	this.tabURL = tabURL;
	this.action = action;
	this.version = version;
}

TabBuffer.prototype.sendData = function () {
	if (this.data.length > 0)
		return this.sendNextChunk();
};

TabBuffer.prototype.sendNextChunk = function () {
	const chL = DataTransmitter.chunkLength;
	if (this.data.length - this.sentBytes > chL) {
		sendNativeMessage(this.tabID, {
			"action": this.action,
			"version": this.version,
			"data": this.data.substring(this.sentBytes, this.sentBytes + chL),
			"title": this.tabTitle,
			"url": this.tabURL,
			"dataRest": this.data.length - this.sentBytes - chL
		});
		this.sentBytes += chL;
		return 1;
	}
	else {
		sendNativeMessage(this.tabID, {
			"action": this.action,
			"version": this.version,
			"data": this.data.substring(this.sentBytes),
			"title": this.tabTitle,
			"url": this.tabURL,
			"dataRest": 0
		});
		this.data = "";
		this.sentBytes = 0;
		return 0;
	}
}

const DataTransmitter = {

	//chunkLength: 10485760, // 10 MB
	chunkLength: 1048576, // 1 MB

	buffers: {},

	sendData: function (tabID, dataURL, tabTitle, tabURL, action, version)
	{
		const buffer = new TabBuffer(tabID, dataURL, tabTitle, tabURL, action, version);
		if (1 === buffer.sendData())
			// We will need it again..
			this.buffers[tabID] = buffer;
	},

	sendNextChunk: function (tabID) {
		if (typeof this.buffers[tabID] !== 'undefined')
			if (0 === this.buffers[tabID].sendNextChunk())
				// all data has been sent
				delete this.buffers[tabID]
	}
}
// Chunks buffer

async function OnTabsOpenPDF(tabs)
{
	const tabID = tabs[0].id;
	const tabTitle = tabs[0].title;
	const tabURL = tabs[0].url;
	
	if (tabURL.startsWith('file:///')) {
		// Older versions do not support some actions
		if (NativeHostVersion > 1) {
			// handle local file
			(await ProgressesManager.getProgressForTab(tabID)).setProgress('OpeningPDFDocument', -1, false);
			sendNativeMessage(tabID, {
				"action": "openLocalPDF",
				"version": "1.0",
				"localPath": tabURL.replace(/^file:\/\/\//, '')
			});
		} else {
			// Here we can not proceed..
			chrome.windows.create({
				type: 'popup',
				url: chrome.runtime.getURL('update-prompt/update-prompt.html'),
				height: 115,
				width: 490,
				focused: true
			});
		}
	} else {
		// handle remmote file. Download it first
		(await ProgressesManager.getProgressForTab(tabID)).setProgress('DownloadingPDFDocument', 0);
		fetch(tabURL).then(async function (response) {
			const reader = response.body.getReader();
			const contentLength = +response.headers.get('Content-Length');

			let receivedLength = 0; // received that many bytes at the moment
			let chunks = []; // array of received binary chunks (comprises the body)
			const progress = await ProgressesManager.getProgressForTab(tabID);
			while (true) {
				try {
					const { done, value } = await reader.read();
					if (done) {
						progress.setProgress('OpeningPDFDocument', -1, false);
						break;
					}
					chunks.push(value);
					receivedLength += value.length;
					let progressV = contentLength > 0 ? Math.round(receivedLength / contentLength * 100) : -1;
					progress.setProgress("DownloadingPDFDocument", progressV, true);
				} catch (err) {
					reportOperationFailure(tabID, { state: 1, message: `PDF document dowloading has failed: "Failed to read next chunk"` });
					break;
				}
			}

			// Concatenate chunks into single Uint8Array
			let chunksAll = new Uint8Array(receivedLength); // (4.1)
			let position = 0;
			for (let chunk of chunks) {
				chunksAll.set(chunk, position); // (4.2)
				position += chunk.length;
			}
			const fileReader = new FileReader();
			fileReader.addEventListener('loadend', (loadEndEvent) => {
				DataTransmitter.sendData(tabID, loadEndEvent.target.result, tabTitle, tabURL, "openPDF", 3.0);
			});
			fileReader.readAsDataURL(new Blob([chunksAll.buffer]));
		}).catch(err => {
			/*
			 * @todo: implement process cancelation....
			if (err.name === 'AbortError') {
				console.log('Fetch aborted');
			} else {
				console.error('Uh oh, an error!', err);
			}
			*/
			// Report error downloading...
			console.error('PDF document dowloading has failed:', err);
			reportOperationFailure(tabID, { state: 1, message: 'PDF document dowloading has failed' });
		});
	}
};

function doOpenPDF(message, sender, sendResponse) {
	chrome.tabs.query({ currentWindow: true, active: true }, OnTabsOpenPDF);
};

function doOnSharepointAddinDocURL(message, sender, sendResponse)
{
	print_debug('doOnSharepointAddinDocURL() call with URL', message.url);
	chrome.tabs.query({ currentWindow: true, active: true }, async (tabs) => {
		const tabID = tabs[0].id;
		(await ProgressesManager.getProgressForTab(tabID)).setProgress('OpeningPDFDocument', -1, false);
		sendNativeMessage(tabID, { "action": "openByURL", "version": 1.0, "URL": message.url });
	});
}

class OperationProgress
{
	finishState = 0;

	constructor(tabID) {
		print_debug('OperationProgress instance has been created with tab #', tabID);
		this.tabID = tabID;
		this.action = '';
		this.progress = -1;
		this.canBeCanceled = false;
		this.finishState = 0;
	}

	static createStorageKey(tabID) {
		return 'pdf-xchange-progress_' + tabID + '_tab';
	}

	setProgress(action, progress, /* bool */canBeCanceled = false, doNotPersist = false, finishState=0) {
		if (typeof action === 'string') {
			this.action = action;
		} else {
			console.warn('Type of action passed is not string', action);
			this.action = '';
		}
		this.progress = progress;
		this.canBeCanceled = canBeCanceled;
		this.finishState = finishState;
		if (!doNotPersist)
			this.#persist();
		this.#notify();
	}

	setCanBeCanceled(flag) {
		this.canBeCanceled = flag;
		this.#persist();
		this.#notify();
	}

	getPayLoad() {
		return {
			action: chrome.i18n.getMessage(this.action),
			progress: this.progress,
			canBeCanceled: this.canBeCanceled,
			can_be_canceled: !!this.canBeCanceled,
			finish_state: this.finishState
		}
	}

	#notify() {
		print_debug('OperationProgress.#notify() is called, this.tabID=', this.tabID);
		chrome.tabs.sendMessage(this.tabID, Object.assign(
			{},
			{ type: "pdf-x-change.progress.post" },
			this.getPayLoad()
		)).catch(err => {
			console.warn('OperationProgress.#notify()::chrome.runtime.sendMessage has failed ', err);
		});
	}

	#persist() {
		chrome.storage.local.set({
			[OperationProgress.createStorageKey(this.tabID)]: this.getPayLoad()
		});
	}

	delete(clb) {
		chrome.storage.local.remove(OperationProgress.createStorageKey(this.tabID), () => {
			if (clb)
				clb();
		});
	}
}

const ProgressesManager =
{
	msgListenerIsInitialized: false,
	progresses: {},

	_deleted: [],

	/**
	 * Creates (if necessary) and returns the OperationProgress instance that reprecents the progress in the specified tab
	 * 
	 * @param {any} tabID
	 * @returns {OperationProgress}
	 */
	getProgressForTab: async function (tabID, createIfNotExist = true) {
		if (this.progresses[tabID]) {
			return this.progresses[tabID];
		} else {
			// look in storage...
			const storageKey = OperationProgress.createStorageKey(tabID);
			let sr;
			try {
				sr = await chrome.storage.local.get(storageKey);
			} catch (err) {
				console.error(`chrome.storage.local.get() for storage key ${storageKey} has failed`, err);
				return null;
			}
			if (sr[storageKey]) {
				const progress = new OperationProgress(tabID);
				progress.setProgress(sr[storageKey].action, sr[storageKey].progress, sr[storageKey].canBeCanceled, true)
				this.progresses[tabID] = progress;
				return progress;
			} else {
				if (createIfNotExist) {
					const progress = new OperationProgress(tabID);
					this.progresses[tabID] = progress;
					return progress;
				} else {
					return null;
				}
			}
		}
	},

	stopProgressForTab: function (tabID, finishState=0) {
		if (this.progresses[tabID]) {
			const progress = this.progresses[tabID];
			progress.setProgress('', 'done', false, true, finishState);
			progress.delete(() => {
				delete this.progresses[tabID];
				this._deleted.push(tabID);
			})
		} else {
			if (-1 === this._deleted.indexOf(tabID))
				console.warn(`Trying to stop progress for tab ${tabID}, while it has never been started, or already stopped.`);
		}
	},

	initializeMsgListener: function() {
		if (this.msgListenerIsInitialized)
			return;
		// The following is being done because the initial progress can be sent before the iframe make it to be created and rendered
		// So once the progress iframe dialog is fully created and loaded it asks for status.
		// @see popup/index.js
		chrome.runtime.onMessage.addListener(function (request, sender, sendResponse) {
			if (request.type && request.tab_id) {
				switch (request.type) {
					case 'pdf-x-change.progress.get': {
						ProgressesManager.getProgressForTab(request.tab_id, false).then((progress) => {
							if (progress) {
								sendResponse(Object.assign(
									{},
									{ type: "pdf-x-change.progress.post", tabID: progress.tabID },
									progress.getPayLoad()
								));
							} else {
								sendResponse(null);
							}
						});
						return true;
					}
					case 'pdf-x-change.cancel_op': {
						ProgressesManager.getProgressForTab(request.tab_id, false).then((progress) => {
							if (progress)
								progress.setProgress('Canceling', -1);
						});
						cancelNativeAction(request.tab_id);
						break;
					}
				}
			}
			// Returning "True" here causes error "Uncaught (in promise) Error: A listener indicated an asynchronous response by returning true, but the message channel closed before a response was received"
			// return true;
		});
		this.msgListenerIsInitialized = true;
	}
}


function reportOperationFailure(tabID, payload) {
	chrome.runtime.sendMessage(Object.assign({}, {
		tabID: tabID,
		type: "pdf-xchange.operation_failed",
	}, payload)).catch(err => {
		console.warn('reportOperationFailure()::chrome.runtime.sendMessage() has failed ', err);
	});
	if (POPUP_TYPE === POPUP_TYPE_IFRAME) {
		const theMessage = Object.assign({}, {
			type: "pdf-xchange.operation_failed",
		}, payload);
		print_debug('Message, sent to tab #', tabID, theMessage);
		chrome.tabs.sendMessage(tabID, theMessage).catch(err => {
			console.warn('reportOperationFailure()::chrome.tabs.sendMessage() has failed ', err);
		});
	}
}

function enablePopup() {
	chrome.action.setIcon({path:{
			"16": "images/ico.app.16.png",
			"24": "images/ico.app.24.png",
			"32": "images/ico.app.32.png",
			"48": "images/ico.app.48.png",
			"64": "images/ico.app.64.png",
			"72": "images/ico.app.72.png",
			"96": "images/ico.app.96.png",
			"128": "images/ico.app.128.png"
		}});
}

function disablePopup() {
	chrome.action.setIcon({path:{
		"16": "images/ico.app.disabled.16.png",
		"24": "images/ico.app.disabled.24.png",
		"32": "images/ico.app.disabled.32.png",
		"48": "images/ico.app.disabled.48.png",
		"64": "images/ico.app.disabled.64.png",
		"72": "images/ico.app.disabled.72.png",
		"96": "images/ico.app.disabled.96.png",
		"128": "images/ico.app.disabled.128.png"
	}});
}

function doOnDocumentLoaded(request, sender) {
	print_debug(`Document has just been loaded in the tab #${sender.tab.id}; active: `, sender.tab.active);
	if (sender.tab.active) {
		if (!sender.tab.url.startsWith("chrome://") && !sender.tab.url.startsWith("edge://")) {
			chrome.action.enable(sender.tab.id, () => {
				print_debug('chrome.action.enable call for tab #', sender.tab.id, 'in doOnDocumentLoaded()');
			});
			enablePopup();
		} else {
			chrome.action.disable(sender.tab.id, () => {
				print_debug('chrome.action.disable call for tab #', sender.tab.id, 'in doOnDocumentLoaded()');
			});
			disablePopup();
		}
	}

	if (request.doctype == "application/pdf") {
		chrome.storage.sync.get(['showOpenPDFFrame'], function (result) {
			chrome.tabs.sendMessage(sender.tab.id, {
				content_op: "showOpenPDFIFrame",
				show: (typeof result.showOpenPDFFrame === "undefined") ? true : result.showOpenPDFFrame
			}).catch((err) => {
				console.warn(
					`Failed to send message to a tab ${sender.tab.id} with just loaded PDF document to show "Open in Editor" widget :: promice rejected`,
					err
				);
			});
		});
	}

	if (POPUP_TYPE === POPUP_TYPE_IFRAME
		&& !sender.tab.url.startsWith("chrome://")
		&& !sender.tab.url.startsWith("edge://")
	) {
		const tabInfoStorageKey = getTabInfoStorageKey(sender.tab.id);
		chrome.storage.local.set({
			[tabInfoStorageKey]: {
				tabId: sender.tab.id,
				docType: request.doctype
			}
		});
	}
}

function doOnShowPDFFrame(message, sender, sendResponse) {
	chrome.storage.sync.set({ showOpenPDFFrame: message.show }, () => { });
	chrome.tabs.query({ currentWindow: true, active: true }, (tabs) => {
		chrome.tabs.sendMessage(tabs[0].id, { content_op: "showOpenPDFIFrame", show: message.show });
	})
}

chrome.tabs.onActivated.addListener(
	async (activeInfo) => {
		const tabID = activeInfo.tabId;
		const tab = await chrome.tabs.get(tabID);
		if (tab.status == 'complete'
			&& !tab.url.startsWith("chrome://")
			&& !tab.url.startsWith("edge://")
		) {
			chrome.action.enable(tab.id, () => {
				print_debug('chrome.action.enable call for tab #', tab.id, 'in chrome.tabs.onActivated listener');
			});
			enablePopup();
		} else {
			chrome.action.disable(tab.id, () => {
				print_debug('chrome.action.disable call for tab #', tab.id, 'in chrome.tabs.onActivated listener');
			});
			disablePopup();
		}

		/* The following can not be used due to bug in Chrome documentation
		   https://github.com/GoogleChrome/developer.chrome.com/issues/2602
		if (await ProgressesManager.getProgressForTab(tabID, false)) {
			console.log('is tab', typeof chrome.action.openPopup);
			if (chrome.action.openPopup)
				chrome.action.openPopup();
		}
		*/
	}
);

chrome.tabs.onRemoved.addListener(async (tabID) => {
	const progress = await ProgressesManager.getProgressForTab(tabID, false);
	if (progress) {
		if ('number' === typeof progress.progress) {
			chrome.windows.create({
				type: 'popup',
				url: chrome.runtime.getURL('op-in-progress-confirm/index.html?p=' + encodeURIComponent(JSON.stringify({
					tab_id: tabID,
					action: progress.getPayLoad().action
				}))),
				height: 145,
				width: 400,
				focused: true
			})
		}
		ProgressesManager.stopProgressForTab(tabID);
	}
	chrome.storage.local.remove(getTabInfoStorageKey(tabID));
})

chrome.tabs.onUpdated.addListener(function (tabId, changeInfo) {
	print_debug(`chrome.tabs.onUpdated listener ia calling for tab #${tabId} with status "${changeInfo.status}"`);
	switch (changeInfo.status) {
		case 'loading':
			chrome.action.disable(tabId, () => {
				print_debug('chrome.action.disable call for tab #', tabId, 'in chrome.tabs.onUpdated listener');
			});
			disablePopup();
			break;
		case 'complete':
			chrome.tabs.get(tabId).then(tab => {
				if (!tab.url.startsWith("chrome://") && !tab.url.startsWith("edge://")) {
					chrome.action.enable(tabId, () => {
						print_debug('chrome.action.enable call for tab #', tabId, 'in chrome.tabs.onUpdated listener');
					});
					enablePopup();
				}
			});
			break;
	}
	if (changeInfo.url) {
		ProgressesManager.getProgressForTab(tabId, false).then(progress => {
			if (progress) {
				if ('number' === typeof progress.progress) {
					chrome.windows.create({
						type: 'popup',
						url: chrome.runtime.getURL('op-in-progress-confirm/index.html?p=' + encodeURIComponent(JSON.stringify({
							tab_id: tabId,
							action: progress.getPayLoad().action
						}))),
						height: 145,
						width: 400,
						focused: true
					})
				}
				ProgressesManager.stopProgressForTab(tabId);
			}
		});
	}
})

chrome.runtime.onInstalled.addListener((details) => {
	const menuItemId = 'doHTMLToPDF';
	const createProperties = {
		// id: menuItemId,
		title: chrome.i18n.getMessage("ConvertToPDFCommand"),
		documentUrlPatterns: [
			"http://*/*",
			"https://*/*",
			"file://*/*",
			"ftp://*/*",
			"urn:*"
		]
	}

	// Added the condition, "'install' === details.reason" as seen error
	// "Unchecked runtime.lastError: Cannot create item with duplicate id doHTMLToPDF"
	// Hope that would stop calling chrome.contextMenus.create() when the context menu item is there and therefore no such errors
	if ('install' === details.reason) {
		chrome.contextMenus.create(Object.assign({}, createProperties, { id: menuItemId }), () => {
			if (chrome.runtime.lastError) {
				console.error('An error occured while context menu create() method call', chrome.runtime.lastError)
			}
		});
	} else if ('update' === details.reason) {
		chrome.contextMenus.update(menuItemId, createProperties, () => {
			if (chrome.runtime.lastError) {
				if (chrome.runtime.lastError.message && 0 === chrome.runtime.lastError.message.indexOf('Cannot find menu item with id')) {
					chrome.contextMenus.create(Object.assign({}, createProperties, { id: menuItemId }), () => {
						if (chrome.runtime.lastError) {
							console.error('An error occured while context menu create() method call', chrome.runtime.lastError)
						}
					});
				} else {
					console.error('An error occured while context menu update() method call', chrome.runtime.lastError)
				}
			}
		});
	}
});

chrome.contextMenus.onClicked.addListener((info) => {
	if ('doHTMLToPDF' == info.menuItemId) {
		chrome.tabs.query({ currentWindow: true, active: true }, (tabs) => {
			if (0 == tabs.length)
				return;
			const tabId = tabs[0].id;
			doHTMLToPDF({ tab_id: tabId });

			if (POPUP_TYPE === POPUP_TYPE_IFRAME) {
				const storageKey = getTabInfoStorageKey(tabId);
				chrome.storage.local.get(storageKey, (tabInfo) => {
					if (tabInfo) {
						// The word "Pseudo" in the operation name means that the popup is not a browser popup, 
						// but actually the iframe, injected into the opened document
						chrome.tabs.sendMessage(tabId, {
							content_op: 'showBrowserActionPseudoPopup',
							doctype: tabInfo[storageKey].docType,
							tab_id: tabId,
							init_with_progress: true
						});
					}
				});
			}
		});
	}
});

if (POPUP_TYPE === POPUP_TYPE_IFRAME) {
	chrome.action.setPopup({ popup: '' });
	chrome.action.onClicked.addListener(function (tab) {
		print_debug(`chrome.action.onClicked listener has been triggered for tab #`, tab.id);
		const storageKey = getTabInfoStorageKey(tab.id);
		chrome.storage.local.get(storageKey, function (tabInfo) {
			if (tabInfo
				&& typeof tabInfo[storageKey] !== 'undefined'
				&& !tab.url.startsWith("chrome://")
				&& !tab.url.startsWith("edge://")
			) {
				print_debug('tabInfo fethed in chrome.action.onClicked listener :: ', tabInfo);
				chrome.tabs.sendMessage(tab.id, {
					content_op: 'showBrowserActionPseudoPopup',
					doctype: tabInfo[storageKey].docType,
					tab_id: tab.id
				}).catch(err => {
					console.warn('chrome.tabs.sendMessage with content_op "showBrowserActionPseudoPopup" has failed ', err);
				});
			}
		});
	});
}

chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
	const handlers = {
		convertHTMLToPDF         : doHTMLToPDF,
		preferences              : doPreferences,
		openPDF                  : doOpenPDF,
		sharepoint_addin_doc_url : doOnSharepointAddinDocURL,
		documentLoaded           : doOnDocumentLoaded,
		showOpenPDFFrame         : doOnShowPDFFrame
	}
	if (message.operation) {
		if (handlers[message.operation]) {
			handlers[message.operation](message, sender, sendResponse);
		}
		else {
			console.warn(`handler for operation ${message.operation} is null`);
		}
	}
	ProgressesManager.initializeMsgListener();

	// Returning "True" here causes error "Uncaught (in promise) Error: A listener indicated an asynchronous response by returning true, but the message channel closed before a response was received"
	// return true;
});
