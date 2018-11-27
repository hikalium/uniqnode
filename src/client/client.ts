import {KnownNodeId} from '../knownids.js';

class NodeData {
  id: string;
  content_type: string;
  content: string;
  constructor(id: string, content_type: string, content: string) {
    this.id = id;
    this.content_type = content_type;
    this.content = content;
  }
}

type CreateNodeDataCallback = (n: NodeData) => void;
export function CreateNodeData(
    id: string, jn: JQuery, callback: CreateNodeDataCallback) {
  $.get(`/uniqnode/v0/node/${id}`, (data: any) => {
    var n = new NodeData(data.id, data.content_type, data.content);
    callback(n);
  });
}

type NodeViewerFunc = (entry: JQuery, n: NodeData) => void;
var nodeViewers: {[key: string]: NodeViewerFunc} = {};

nodeViewers[KnownNodeId.kStringEncodedAsURIComponent] = function(
    entry: JQuery, n: NodeData) {
  entry.find('.card-title').text(decodeURIComponent(n.content));
  entry.find('.card-title').append(`<small><span class="badge badge-pill badge-primary ml-2">text</span></small>`);
};

function syncNodes(nodeListDiv: JQuery) {
  nodeListDiv.empty();
  var createIdEntry = function(id: string) {
    return $(` 
      <div class="col-sm-12">
        <div class="card">
          <div class="card-body">
            <h5 class="card-title">${id}</h5>
            <div class="uniqnode-id">${id}</div>
            <div class="uniqnode-contents"></div>
          </div>
        </div>
      </div>
      `);
  };
  $.get(`/uniqnode/v0/node`, (ids: string[]) => {
    for (var id of ids) {
      let entry = createIdEntry(id);
      nodeListDiv.append(entry);
      CreateNodeData(id, entry, (n) => {
        if (nodeViewers[n.content_type]) {
          nodeViewers[n.content_type](entry, n);
        }
      });
    }
  });
}

$(window).on('load', function() {
  $('#syncButton').click((event) => {
    syncNodes($('#nodeListDiv'));
  })
  $('#addTextButton').click((event) => {
    var str: string|null = <string|null>$('#textInput').val();
    if (!str) return;
    $.post('/uniqnode/v0/node', {
      content_type: 'e11708cd4bdf45c28732e47153dda9a9',
      content: encodeURIComponent(str),
    });
  })
  syncNodes($('#nodeListDiv'));
});
