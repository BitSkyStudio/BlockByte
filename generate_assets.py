import shutil
import subprocess
import os
bbtpl_path = shutil.which("bbtpl")
asset_dir = os.path.join(os.getcwd(), "assets_generated")
print("bbtpl: " + bbtpl_path)
print("assets: " + asset_dir)
shutil.rmtree(asset_dir)
os.mkdir(asset_dir)

def generate(name, params):
    arguments = [bbtpl_path, name]
    for key, value in params.items():
        arguments.append(str(key) + "=" + str(value))
    subprocess.Popen(arguments, cwd=asset_dir)
def generate_tag(name, content):
    with open(os.path.join(asset_dir, *name.split(".")) + ".txt", "w") as f:
        f.write("\n".join(content))

wood_types = [
    {"woodType": "oak", "barkColor": "574026", "woodColor": "8c6d47"},
    {"woodType": "acacia", "barkColor": "754a08", "woodColor": "ba8457"},
    {"woodType": "mangrove", "barkColor": "4a2907", "woodColor": "66482a"},
    {"woodType": "cedar", "barkColor": "4d4339", "woodColor": "706254"},
    {"woodType": "willow", "barkColor": "6e665e", "woodColor": "918579"},
    {"woodType": "apple", "barkColor": "542a05", "woodColor": "693b13"},
    {"woodType": "cherry", "barkColor": "291b10", "woodColor": "8d7296"},
    {"woodType": "maple", "barkColor": "5c3e24", "woodColor": "825834"},
    {"woodType": "pine", "barkColor": "4d381e", "woodColor": "8a602c"},
]
for data in wood_types:
    generate("bb:wood_type", data)
generate_tag("#sticks", ["wood." + data["woodType"] + ".stick" for data in wood_types])

rock_types = [
    {"rockType": "chalk", "rockColor": "9f9986"},
    {"rockType": "limestone", "rockColor": "7b7b71"},
    {"rockType": "sandstone", "rockColor": "7b7542"},
    {"rockType": "shale", "rockColor": "6e6a5f"},
    {"rockType": "diorite", "rockColor": "575b5c"},
    {"rockType": "granite", "rockColor": "b2845f"},
    {"rockType": "andesite", "rockColor": "7f7e6e"},
    {"rockType": "basalt", "rockColor": "45423b"},
    {"rockType": "marble", "rockColor": "bdb6a4"},
    {"rockType": "slate", "rockColor": "996046"},
    {"rockType": "gneiss", "rockColor": "876860"},
    {"rockType": "claystone", "rockColor": "716b5c"},
]
for data in rock_types:
    generate("bb:rock_type", data)

ore_types = [{"oreType": "magnetite", "oreColor": "646464"}]
for ore_data in ore_types:
    break
    generate("bb:ore_type", ore_data)
    for rock_data in rock_types:
        data = dict(ore_data)
        data.update(rock_data)
        generate("bb:rock_ore", data)