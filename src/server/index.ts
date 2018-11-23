import express = require('express');
import * as MongoDb from 'mongodb';
import {KnownNodeId} from '../knownids';
require('dotenv').config();
const assert = require('assert');
const bodyParser = require('body-parser')
const {exec} = require('child_process');
const uuidv4 = require('uuid/v4');
const app = express();
const port = 3000;
const dbUrl = `mongodb://${process.env.DB_HOST}:${process.env.DB_PORT}`;
const dbName = `${process.env.DB_NAME}`;

var db;
var nodes: MongoDb.Collection;
var rels: MongoDb.Collection;

function generateId() {
  return uuidv4().replace(/-/g, '');
}

app.set('view engine', 'ejs');

app.use(express.static('static'));
app.use(express.static('out/client'));
app.use(bodyParser.json());
app.use(bodyParser.urlencoded({extended: true}));

app.get('/', (req, res) => {
  nodes.find().toArray(function(err: Error, result: any[]) {
    if (err) throw err;
    console.log(result);
    res.render('index', {nodes: result});
  });
})

app.get('/init', (req, res) => {
  nodes.deleteMany({});
  rels.deleteMany({});
  nodes.insertOne({id: generateId()});
  res.json({status: 'OK'});
})

app.get('/uniqnode/v0/node/:id', (req, res) => {
  nodes.findOne({id: req.params.id}).then((result) => {
    console.log(result);
    res.json({
      id: result.id,
      content_type: result.content_type,
      content: result.content
    });
  });
})

app.post('/uniqnode/v0/node', (req, res) => {
  const insertData = {
    id: generateId(),
    content_type: req.body.content_type,
    content: req.body.content
  };
  nodes.insertOne(insertData, (db_err, db_res) => {
    if (db_err) {
      res.status(500).json({err: db_err}).end();
    } else {
      res.json({
        id: insertData.id,
        content_type: insertData.content_type,
        content: insertData.content
      });
    }
  });
})

const client = new MongoDb.MongoClient(dbUrl, {useNewUrlParser: true});

client.connect(function(err: Error) {
  assert.equal(null, err);
  console.log('Connected successfully to server');
  db = client.db(dbName);
  db.collection('nodes', (err, collection) => {
    assert.equal(null, err);
    nodes = collection;
  });
  db.collection('rels', (err, collection) => {
    assert.equal(null, err);
    rels = collection;
  });
});


app.listen(port, () => console.log(`uniqnode listening on port ${port}!`))
